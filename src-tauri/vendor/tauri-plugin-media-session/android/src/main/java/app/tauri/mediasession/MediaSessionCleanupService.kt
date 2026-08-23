package app.tauri.mediasession

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service that keeps the process alive while media is actively playing.
 *
 * Acquired on play, released on stop/clear/task-removed:
 * - Foreground service: prevents process kill and network throttling
 * - PARTIAL_WAKE_LOCK: keeps CPU alive so native playback / tick can run between tracks
 * - AUDIO_BECOMING_NOISY receiver: pauses when headphones are unplugged
 *
 * Important lifecycle rules (Android 8–14):
 * - Never cold-start this service while paused — mediaPlayback FGS without active
 *   playback is rejected / unstable and was crashing Wave on cold launch.
 * - Always call startForeground() before stopSelf() if we were started via
 *   startForegroundService(), or the whole process is killed.
 * - Prefer START_NOT_STICKY so a dead process is not sticky-restarted into a
 *   no-notification crash loop.
 */
class MediaSessionCleanupService : Service() {

    companion object {
        private const val TAG = "plugin/media-session"
        private const val ACTION_INIT = "app.tauri.mediasession.ACTION_INIT"
        internal const val NOTIFICATION_ID = 9401

        @Volatile internal var instance: MediaSessionCleanupService? = null
        @Volatile internal var pendingNotification: Notification? = null
        @Volatile private var safeModeLogged = false

        /**
         * Start (or update) the foreground service with the given notification.
         * Cold-start is disabled in safe mode — it was crashing cold launch on
         * Android 14+ after the post-v0.3.0 media-session changes. Existing
         * service instances can still update their notification.
         */
        fun start(context: Context, notification: Notification) {
            if (!MediaSessionState.sessionActive) {
                Log.d(TAG, "start: session inactive, ignoring")
                return
            }
            pendingNotification = notification
            val svc = instance
            if (svc != null) {
                try {
                    svc.ensureForeground(notification)
                } catch (t: Throwable) {
                    Log.e(TAG, "ensureForeground failed: ${t.message}")
                    recordHostError(context, "MediaSessionCleanupService.ensureForeground", t)
                }
                return
            }
            // SAFE MODE: do not call startForegroundService. A failed / late
            // startForeground kills the entire process; CrashReporter keeps a
            // note so the next successful UI launch can show what happened.
            Log.w(TAG, "start: safe-mode skip cold-start FGS")
            if (!safeModeLogged) {
                safeModeLogged = true
                recordHostMessage(
                    context,
                    "MediaSessionCleanupService.start",
                    "Skipped media foreground service cold-start (safe mode). "
                        + "App should stay open; lock-screen / shade controls may be missing until re-enabled."
                )
            }
        }

        private fun recordHostError(context: Context, where: String, error: Throwable) {
            try {
                Class.forName("app.bmdarklight.wave.CrashReporter")
                    .getMethod(
                        "recordError",
                        Context::class.java,
                        String::class.java,
                        Throwable::class.java
                    )
                    .invoke(null, context, where, error)
            } catch (_: Throwable) {
            }
        }

        private fun recordHostMessage(context: Context, where: String, message: String) {
            try {
                Class.forName("app.bmdarklight.wave.CrashReporter")
                    .getMethod(
                        "recordMessage",
                        Context::class.java,
                        String::class.java,
                        String::class.java
                    )
                    .invoke(null, context, where, message)
            } catch (_: Throwable) {
            }
        }

        fun stop() {
            instance?.handleStop()
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var noisyReceiver: BroadcastReceiver? = null
    private var foregroundStarted = false
    private val keepaliveHandler = Handler(Looper.getMainLooper())
    private val keepaliveIntervalMs = 15_000L
    private val keepaliveRunnable = object : Runnable {
        override fun run() {
            if (!MediaSessionState.sessionActive) return
            if (MediaSessionPlugin.isPlaybackActive() && MediaSessionState.hasActiveMedia()) {
                MediaSessionState.refreshForeground(applicationContext, advancePosition = true)
            }
            if (MediaSessionState.sessionActive && foregroundStarted) {
                keepaliveHandler.postDelayed(this, keepaliveIntervalMs)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        instance = this
        Log.d(TAG, "onCreate")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            var notification = pendingNotification
            if (notification == null
                && MediaSessionState.sessionActive
                && MediaSessionState.isPlaying
                && MediaSessionState.hasActiveMedia()
            ) {
                Log.w(TAG, "onStartCommand: rebuilding notification for active playback")
                try {
                    MediaSessionState.refreshForeground(applicationContext, advancePosition = true)
                } catch (t: Throwable) {
                    Log.e(TAG, "rebuild notification failed: ${t.message}")
                }
                notification = pendingNotification
            }

            if (notification == null) {
                Log.w(TAG, "onStartCommand: no media — shutting down safely")
                shutDownGracefully()
                return START_NOT_STICKY
            }

            ensureForeground(notification)
            acquireWakeLock()
            registerNoisyReceiver()
            keepaliveHandler.removeCallbacks(keepaliveRunnable)
            keepaliveHandler.postDelayed(keepaliveRunnable, keepaliveIntervalMs)
            Log.d(TAG, "Foreground started, locks acquired")
        } catch (t: Throwable) {
            Log.e(TAG, "onStartCommand crashed: ${t.message}", t)
            try {
                shutDownGracefully()
            } catch (t2: Throwable) {
                Log.e(TAG, "shutdown after crash failed: ${t2.message}")
                try {
                    stopSelf()
                } catch (_: Throwable) {
                }
            }
        }
        // Never START_STICKY — sticky restarts without a notification kill the
        // process on the next cold open (ForegroundServiceDidNotStartInTime).
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onTaskRemoved(rootIntent: Intent?) {
        Log.d(TAG, "onTaskRemoved — stopping playback")
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        pendingNotification = null
        try {
            MediaSessionPlugin.forceCleanup(applicationContext)
        } catch (t: Throwable) {
            Log.e(TAG, "forceCleanup failed: ${t.message}")
        }
        shutDownGracefully()
    }

    override fun onDestroy() {
        Log.d(TAG, "onDestroy")
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        instance = null
        releaseResources()
        if (!MediaSessionState.sessionActive || !MediaSessionPlugin.isPlaybackActive()) {
            try {
                MediaSessionPlugin.cancelNotificationArtifactsOnly(applicationContext)
            } catch (_: Throwable) {
            }
        }
        super.onDestroy()
    }

    internal fun postNotification(notification: Notification) {
        ensureForeground(notification)
    }

    internal fun cancelKeepalive() {
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
    }

    private fun ensureForeground(notification: Notification) {
        pendingNotification = notification
        var started = false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            try {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
                )
                started = true
            } catch (t: Throwable) {
                Log.w(TAG, "startForeground(mediaPlayback) failed: ${t.message}")
            }
        }
        if (!started) {
            @Suppress("DEPRECATION")
            startForeground(NOTIFICATION_ID, notification)
        }
        foregroundStarted = true
        try {
            val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            nm.notify(NOTIFICATION_ID, notification)
        } catch (t: Throwable) {
            Log.w(TAG, "notify failed: ${t.message}")
        }
    }

    private fun shutDownGracefully() {
        try {
            if (!foregroundStarted) {
                ensureForeground(buildPlaceholderNotification())
            }
        } catch (t: Throwable) {
            Log.w(TAG, "placeholder startForeground failed: ${t.message}")
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
        } catch (_: Throwable) {
        }
        foregroundStarted = false
        releaseResources()
        try {
            stopSelf()
        } catch (_: Throwable) {
        }
    }

    private fun buildPlaceholderNotification(): Notification {
        val channelId = "${packageName}.media"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NotificationManager::class.java)
            if (manager?.getNotificationChannel(channelId) == null) {
                manager?.createNotificationChannel(
                    NotificationChannel(
                        channelId,
                        "Media playback",
                        NotificationManager.IMPORTANCE_LOW
                    ).apply { description = "Media playback controls" }
                )
            }
        }
        return NotificationCompat.Builder(this, channelId)
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle("Wave")
            .setContentText("Stopping…")
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun handleStop() {
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        pendingNotification = null
        shutDownGracefully()
    }

    private fun releaseResources() {
        unregisterNoisyReceiver()
        releaseWakeLock()
    }

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        try {
            val pm = getSystemService(POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK,
                "app.tauri.mediasession:PlaybackWakeLock"
            ).apply { acquire(24 * 60 * 60 * 1000L) }
            Log.d(TAG, "WakeLock acquired")
        } catch (t: Throwable) {
            Log.w(TAG, "WakeLock failed: ${t.message}")
        }
    }

    private fun releaseWakeLock() {
        try {
            wakeLock?.let { if (it.isHeld) it.release() }
        } catch (_: Throwable) {
        }
        wakeLock = null
    }

    private fun registerNoisyReceiver() {
        if (noisyReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent?) {
                if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                    Log.d(TAG, "Audio becoming noisy — pausing")
                    MediaSessionPlugin.handleMediaAction("pause")
                }
            }
        }
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        // API 33+ requires an export flag; missing it crashes the process.
        ContextCompat.registerReceiver(
            this,
            receiver,
            filter,
            ContextCompat.RECEIVER_NOT_EXPORTED
        )
        noisyReceiver = receiver
        Log.d(TAG, "Noisy receiver registered")
    }

    private fun unregisterNoisyReceiver() {
        noisyReceiver?.let {
            try {
                unregisterReceiver(it)
            } catch (_: Exception) {
            }
            noisyReceiver = null
        }
    }
}
