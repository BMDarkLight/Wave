package app.tauri.mediasession

import android.app.Notification
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
import android.os.Process
import android.util.Log

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
                svc.postNotification(notification)
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
                return START_STICKY
            }
            Log.w(TAG, "onStartCommand: no notification yet, staying alive")
            stopSelf()
            return START_NOT_STICKY
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID, notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
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
        // User removed Wave from recents — treat as a full exit: stop playback,
        // drop the notification, and end the process. Pressing Home (without
        // swiping away) does not call this, so background playback still works.
        Log.d(TAG, "onTaskRemoved — stopping playback and exiting")
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        pendingNotification = null
        MediaSessionPlugin.forceCleanup(applicationContext)
        stopSelf()
        Process.killProcess(Process.myPid())
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
        val nm = getSystemService(NOTIFICATION_SERVICE) as android.app.NotificationManager
        nm.notify(NOTIFICATION_ID, notification)
    }

    internal fun cancelKeepalive() {
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
    }

    private fun handleStop() {
        keepaliveHandler.removeCallbacks(keepaliveRunnable)
        releaseResources()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        stopSelf()
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
