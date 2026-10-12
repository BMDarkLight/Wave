package app.tauri.mediasession

import android.annotation.SuppressLint
import android.app.Notification
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service that keeps the process alive for playback.
 *
 * - Playing: in the foreground with a partial wake lock, so the engine keeps
 *   running and moves to the next track with the screen off, and Android does
 *   not kill the process when the task is swiped away.
 * - Paused: leaves the foreground but keeps the notification, which becomes
 *   dismissable straight away (Media3 does the same with
 *   `stopForeground(false)`). Resuming from the notification or a media button
 *   comes with a foreground-start exemption from the system.
 * - Held for a call: stays in the foreground, because the resume afterwards
 *   has no user action behind it and would not be allowed to start it again.
 * - Task swiped away: playback keeps going if something is playing; otherwise
 *   the service and notification go away with the app.
 *
 * Only ever driven from the main thread by [MediaSessionState].
 */
class MediaSessionCleanupService : Service() {

    companion object {
        private const val TAG = "plugin/media-session"
        private const val WAKE_LOCK_TIMEOUT_MS = 6 * 60 * 60 * 1000L

        @Volatile
        internal var instance: MediaSessionCleanupService? = null

        private var pending: Notification? = null
        private var startRequested = false

        // True while one of our notifications is on screen.
        private var notificationShown = false

        /** Show [notification], starting or updating the service as needed. */
        internal fun publish(context: Context, notification: Notification, foreground: Boolean) {
            val svc = instance
            if (svc != null) {
                svc.apply(notification, foreground)
                return
            }
            pending = notification
            if (startRequested) return
            if (!foreground) {
                // A paused track with no service (say, the queue restored on
                // launch): only refresh a notification that is already up.
                if (notificationShown) notify(context, notification)
                return
            }
            try {
                ContextCompat.startForegroundService(
                    context,
                    Intent(context, MediaSessionCleanupService::class.java)
                )
                startRequested = true
            } catch (t: Throwable) {
                // Android 12+ refuses a foreground start from the background
                // without a user action behind it. Handled, not a crash:
                // playback still runs, the controls are posted without the
                // foreground guarantee, and the next state change retries.
                Log.w(TAG, "startForegroundService refused: ${t.message}")
                notify(context, notification)
            }
        }

        internal fun stop(context: Context) {
            pending = null
            startRequested = false
            instance?.shutDown()
            cancel(context)
        }

        @SuppressLint("MissingPermission")
        private fun notify(context: Context, notification: Notification) {
            try {
                NotificationManagerCompat.from(context)
                    .notify(MediaSessionState.NOTIFICATION_ID, notification)
                notificationShown = true
            } catch (t: Throwable) {
                // POST_NOTIFICATIONS denied: nothing to show, playback is fine.
                Log.w(TAG, "notify failed: ${t.message}")
            }
        }

        /**
         * Remove the notification and stop the service from any thread, without
         * waiting for the main looper. For the moment before the process exits.
         */
        internal fun shutdownNow(context: Context) {
            pending = null
            startRequested = false
            cancel(context)
            try {
                context.stopService(Intent(context, MediaSessionCleanupService::class.java))
            } catch (_: Throwable) {
            }
        }

        internal fun cancel(context: Context) {
            notificationShown = false
            try {
                NotificationManagerCompat.from(context).cancel(MediaSessionState.NOTIFICATION_ID)
            } catch (_: Throwable) {
            }
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var inForeground = false
    private var everForeground = false

    override fun onCreate() {
        super.onCreate()
        instance = this
        MediaSessionState.init(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startRequested = false
        val notification = pending
        if (notification == null || !MediaSessionState.hasMedia) {
            // Started with nothing to show (a stop raced the start). Android
            // still requires one startForeground call or it kills the process.
            Log.d(TAG, "started without media, shutting down")
            shutDown()
            return START_NOT_STICKY
        }
        apply(notification, MediaSessionState.foregroundWanted)
        // Never sticky: a restart without the app's runtime has nothing to play.
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onTaskRemoved(rootIntent: Intent?) {
        if (MediaSessionState.foregroundWanted || MediaSessionPlugin.isPlaybackActive()) {
            Log.d(TAG, "task removed while playing, playback continues")
            return
        }
        // Paused: the app is going away with the task. Leave the queue alone
        // so it restores on the next launch, just take the controls down.
        Log.d(TAG, "task removed while paused, shutting down")
        pending = null
        shutDown()
        cancel(this)
    }

    override fun onDestroy() {
        releaseWakeLock()
        instance = null
        if (!MediaSessionState.hasMedia) cancel(this)
        super.onDestroy()
    }

    internal fun apply(notification: Notification, foreground: Boolean) {
        pending = notification
        if (foreground) {
            enterForeground(notification)
            acquireWakeLock()
            return
        }

        releaseWakeLock()
        if (!everForeground) {
            // startForegroundService() must be answered with startForeground(),
            // even if playback paused while the service was starting.
            enterForeground(notification)
        }
        // Post first, then detach: the paused notification stays up, now
        // dismissable, and is no longer tied to the service.
        notify(this, notification)
        if (inForeground) stopForegroundCompat(removeNotification = false)
    }

    private fun enterForeground(notification: Notification) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    MediaSessionState.NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
                )
            } else {
                startForeground(MediaSessionState.NOTIFICATION_ID, notification)
            }
            inForeground = true
            everForeground = true
            notificationShown = true
        } catch (t: Throwable) {
            // ForegroundServiceStartNotAllowedException: resumed from the
            // background after the paused window ran out. Keep the controls
            // visible; the next resume from the app or notification recovers.
            Log.w(TAG, "startForeground refused: ${t.message}")
            notify(this, notification)
        }
    }

    private fun stopForegroundCompat(removeNotification: Boolean) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(
                    if (removeNotification) STOP_FOREGROUND_REMOVE else STOP_FOREGROUND_DETACH
                )
            } else {
                @Suppress("DEPRECATION")
                stopForeground(removeNotification)
            }
        } catch (_: Throwable) {
        }
        inForeground = false
        releaseWakeLock()
    }

    internal fun shutDown() {
        if (!everForeground) {
            try {
                enterForeground(placeholderNotification())
            } catch (_: Throwable) {
            }
        }
        stopForegroundCompat(removeNotification = true)
        try {
            stopSelf()
        } catch (_: Throwable) {
        }
    }

    private fun placeholderNotification(): Notification {
        MediaSessionState.ensureChannel(this)
        return NotificationCompat.Builder(this, MediaSessionState.channelId(this))
            .setSmallIcon(MediaSessionState.smallIcon(this))
            .setContentTitle(applicationInfo.loadLabel(packageManager))
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setSilent(true)
            .setShowWhen(false)
            .build()
    }

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        try {
            val pm = getSystemService(POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK,
                "app.tauri.mediasession:Playback"
            ).apply {
                setReferenceCounted(false)
                acquire(WAKE_LOCK_TIMEOUT_MS)
            }
        } catch (t: Throwable) {
            Log.w(TAG, "wake lock failed: ${t.message}")
        }
    }

    private fun releaseWakeLock() {
        try {
            wakeLock?.let { if (it.isHeld) it.release() }
        } catch (_: Throwable) {
        }
        wakeLock = null
    }
}
