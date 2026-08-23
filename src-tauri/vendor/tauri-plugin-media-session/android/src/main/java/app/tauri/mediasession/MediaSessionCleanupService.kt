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

/**
 * Foreground service that keeps the process alive for the entire duration of a media session.
 *
 * Acquired on session start, released only on session clear:
 * - Foreground service: prevents process kill and network throttling
 * - PARTIAL_WAKE_LOCK: keeps CPU alive so native playback / tick can run between tracks
 * - AUDIO_BECOMING_NOISY receiver: pauses when headphones are unplugged
 *
 * Audio focus is owned by WaveExoPlayer (Media3 handleAudioFocus). This service
 * must not also request AUDIOFOCUS_GAIN — doing so steals focus from Exo right
 * after play starts and leaves playback paused until the user retries.
 *
 * Transport actions are dispatched through [MediaSessionPlugin.handleMediaAction],
 * which prefers the host app's native Rust bridge so controls work while the
 * WebView is frozen in the background.
 */
class MediaSessionCleanupService : Service() {

    companion object {
        private const val TAG = "plugin/media-session"
        private const val ACTION_INIT = "app.tauri.mediasession.ACTION_INIT"
        internal const val NOTIFICATION_ID = 9401

        @Volatile internal var instance: MediaSessionCleanupService? = null
        @Volatile internal var pendingNotification: Notification? = null

        /**
         * Start (or update) the foreground service with the given notification.
         * Must be called while the app is in the foreground on first call.
         */
        fun start(context: Context, notification: Notification) {
            if (!MediaSessionState.sessionActive) {
                Log.d(TAG, "start: session inactive, ignoring")
                return
            }
            pendingNotification = notification
            val svc = instance
            if (svc != null) {
                svc.ensureForeground(notification)
            } else {
                try {
                    // Application context survives Activity recreation.
                    val appCtx = context.applicationContext
                    appCtx.startForegroundService(
                        Intent(appCtx, MediaSessionCleanupService::class.java)
                            .setAction(ACTION_INIT)
                    )
                } catch (e: Exception) {
                    Log.e(TAG, "startForegroundService failed: ${e.message}")
                }
            }
        }

        /**
         * Stop the foreground service and release all resources.
         * Safe to call from any context — uses the direct instance reference.
         */
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
                val context = applicationContext
                MediaSessionState.refreshForeground(context, advancePosition = true)
            }
            if (MediaSessionState.sessionActive) {
                keepaliveHandler.postDelayed(this, keepaliveIntervalMs)
            }
        }
    }

    // ── Service lifecycle ────────────────────────────────────────────────────

    override fun onCreate() {
        super.onCreate()
        instance = this
        Log.d(TAG, "onCreate")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = pendingNotification
        if (notification == null) {
            if (MediaSessionState.sessionActive
                && MediaSessionPlugin.isPlaybackActive()
                && MediaSessionState.hasActiveMedia()
            ) {
                Log.w(TAG, "onStartCommand: rebuilding notification for active playback")
                MediaSessionState.refreshForeground(applicationContext, advancePosition = true)
                // refreshForeground → start() → ensureForeground / pendingNotification.
                if (pendingNotification != null) {
                    return START_STICKY
                }
            }
            // Sticky restart after process death, or startForegroundService race:
            // Android requires startForeground() before we can stop, otherwise the
            // whole app is killed with ForegroundServiceDidNotStartInTimeException.
            Log.w(TAG, "onStartCommand: no media to keep alive — shutting down safely")
            shutDownGracefully()
            return START_NOT_STICKY
        }

        ensureForeground(notification)
        acquireWakeLock()
        registerNoisyReceiver()
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        keepaliveHandler.postDelayed(keepaliveRunnable, keepaliveIntervalMs)
        Log.d(TAG, "Foreground started, locks acquired")
        // START_STICKY: ask the system to recreate us after low-memory kills
        // so background playback can recover with the pending notification.
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onTaskRemoved(rootIntent: Intent?) {
        // User removed Wave from recents — treat as a full exit: stop playback
        // and drop the notification. Pressing Home (without swiping away) does
        // not call this, so background playback still works.
        //
        // Do NOT Process.killProcess() here: it races with stopSelf() and leaves
        // a START_STICKY restart that opens, fails startForeground, and crashes
        // the next cold start.
        Log.d(TAG, "onTaskRemoved — stopping playback and exiting")
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        pendingNotification = null
        MediaSessionPlugin.forceCleanup(applicationContext)
        shutDownGracefully()
    }

    override fun onDestroy() {
        Log.d(TAG, "onDestroy")
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        instance = null
        releaseResources()
        // Keep the transport notification while audio is still playing — a sticky
        // service restart should recover via MediaSessionState, not wipe the UI.
        if (!MediaSessionState.sessionActive || !MediaSessionPlugin.isPlaybackActive()) {
            MediaSessionPlugin.cancelNotificationArtifactsOnly(applicationContext)
        }
        super.onDestroy()
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    internal fun postNotification(notification: Notification) {
        ensureForeground(notification)
    }

    internal fun cancelKeepalive() {
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
    }

    private fun ensureForeground(notification: Notification) {
        pendingNotification = notification
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID, notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        foregroundStarted = true
        val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(NOTIFICATION_ID, notification)
    }

    /**
     * Always satisfy the FGS contract before stopping. Calling stopSelf() (or
     * being sticky-restarted) without startForeground() crashes the process.
     */
    private fun shutDownGracefully() {
        try {
            if (!foregroundStarted) {
                ensureForeground(buildPlaceholderNotification())
            }
        } catch (e: Exception) {
            Log.w(TAG, "placeholder startForeground failed: ${e.message}")
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
        } catch (_: Exception) {
        }
        foregroundStarted = false
        releaseResources()
        stopSelf()
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

    // ── WakeLock ─────────────────────────────────────────────────────────────

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "app.tauri.mediasession:PlaybackWakeLock"
        ).apply { acquire(24 * 60 * 60 * 1000L) }
        Log.d(TAG, "WakeLock acquired")
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        Log.d(TAG, "WakeLock released")
    }

    // ── Becoming Noisy (headphone unplug / BT disconnect) ────────────────────

    private fun registerNoisyReceiver() {
        if (noisyReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent?) {
                if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                    Log.d(TAG, "Audio becoming noisy (headphones unplugged) — pausing")
                    MediaSessionPlugin.handleMediaAction("pause")
                }
            }
        }
        registerReceiver(receiver, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY))
        noisyReceiver = receiver
        Log.d(TAG, "Noisy receiver registered")
    }

    private fun unregisterNoisyReceiver() {
        noisyReceiver?.let {
            try { unregisterReceiver(it) } catch (_: Exception) {}
            noisyReceiver = null
            Log.d(TAG, "Noisy receiver unregistered")
        }
    }
}
