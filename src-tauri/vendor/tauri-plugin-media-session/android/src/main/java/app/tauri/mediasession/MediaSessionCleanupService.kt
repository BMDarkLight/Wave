package app.tauri.mediasession

import android.annotation.SuppressLint
import android.app.Notification
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service that keeps the process alive for playback.
 *
 * - Playing: in the foreground with a partial wake lock, so the engine keeps
 *   running and can move to the next track with the screen off.
 * - Paused: stays in the foreground for [PAUSED_FOREGROUND_MS] (the same window
 *   Media3 uses). Resuming inside it never needs a fresh foreground start,
 *   which Android 12+ may refuse while the app is in the background. After
 *   that it leaves the foreground but keeps the notification, which becomes
 *   dismissable.
 * - Task swiped away: playback keeps going if something is playing; otherwise
 *   the service and notification go away with the app.
 *
 * Only ever driven from the main thread by [MediaSessionState].
 */
class MediaSessionCleanupService : Service() {

    companion object {
        private const val TAG = "plugin/media-session"
        private const val PAUSED_FOREGROUND_MS = 10 * 60 * 1000L
        private const val WAKE_LOCK_TIMEOUT_MS = 6 * 60 * 60 * 1000L

        @Volatile
        internal var instance: MediaSessionCleanupService? = null

        private var pending: Notification? = null
        private var startRequested = false

        // True while one of our notifications is on screen.
        private var notificationShown = false

        /** Show [notification], starting or updating the service as needed. */
        internal fun publish(context: Context, notification: Notification, playing: Boolean) {
            val svc = instance
            if (svc != null) {
                svc.apply(notification, playing)
                return
            }
            pending = notification
            if (startRequested) return
            if (!playing) {
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
                // Background start refused (Android 12+). Playback still runs;
                // show the controls without the foreground guarantee.
                Log.w(TAG, "startForegroundService refused: ${t.message}")
                MediaSessionState.recordHostError("MediaSessionCleanupService.start", t)
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

        internal fun cancel(context: Context) {
            notificationShown = false
            try {
                NotificationManagerCompat.from(context).cancel(MediaSessionState.NOTIFICATION_ID)
            } catch (_: Throwable) {
            }
        }
    }

    private val handler = Handler(Looper.getMainLooper())
    private var wakeLock: PowerManager.WakeLock? = null
    private var inForeground = false
    private var everForeground = false

    private val leaveForeground = Runnable {
        if (MediaSessionState.playing || !inForeground) return@Runnable
        Log.d(TAG, "paused for a while, leaving the foreground")
        stopForegroundCompat(removeNotification = false)
        // Repost so the notification picks up its dismissable flags.
        MediaSessionState.republish()
    }

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
        apply(notification, MediaSessionState.playing)
        // Never sticky: a restart without the app's runtime has nothing to play.
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onTaskRemoved(rootIntent: Intent?) {
        if (MediaSessionState.playing || MediaSessionPlugin.isPlaybackActive()) {
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
        handler.removeCallbacks(leaveForeground)
        releaseWakeLock()
        instance = null
        if (!MediaSessionState.hasMedia) cancel(this)
        super.onDestroy()
    }

    internal fun apply(notification: Notification, playing: Boolean) {
        pending = notification
        if (playing) {
            handler.removeCallbacks(leaveForeground)
            enterForeground(notification)
            acquireWakeLock()
            return
        }

        releaseWakeLock()
        if (!everForeground) {
            // startForegroundService() must be answered with startForeground().
            enterForeground(notification)
        } else {
            notify(this, notification)
        }
        if (inForeground) {
            handler.removeCallbacks(leaveForeground)
            handler.postDelayed(leaveForeground, PAUSED_FOREGROUND_MS)
        }
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
            MediaSessionState.recordHostError("MediaSessionCleanupService.startForeground", t)
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
        handler.removeCallbacks(leaveForeground)
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
