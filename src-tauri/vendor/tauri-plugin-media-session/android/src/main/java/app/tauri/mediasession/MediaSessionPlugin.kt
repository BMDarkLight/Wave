package app.tauri.mediasession

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class UpdateStateArgs {
    var title: String? = null
    var artist: String? = null
    var album: String? = null
    var artworkUrl: String? = null
    var duration: Double? = null
    var position: Double? = null
    var playbackSpeed: Double? = null
    var isPlaying: Boolean? = null
    var canPrev: Boolean? = null
    var canNext: Boolean? = null
    var canSeek: Boolean? = null
    var shuffleEnabled: Boolean? = null
    var repeatMode: String? = null
}

@InvokeArg
class UpdateTimelineArgs {
    var position: Double? = null
    var duration: Double? = null
    var playbackSpeed: Double? = null
}

/**
 * Tauri-facing side of the media session.
 *
 * Tauri creates this once per process and ties it to the first Activity, so it
 * holds no session state itself: everything lives in [MediaSessionState],
 * which outlives the Activity. The commands stay for the guest JS API; the
 * Wave engine talks to the session through [MediaSessionBridge] instead.
 */
@TauriPlugin
class MediaSessionPlugin(private val activity: Activity) : Plugin(activity) {

    private var notificationPermissionRequested = false

    init {
        activeInstance = this
        MediaSessionState.init(activity.applicationContext)
        try {
            Class.forName("app.bmdarklight.wave.CrashReporter")
                .getMethod("install", Context::class.java)
                .invoke(null, activity.applicationContext)
        } catch (_: Throwable) {
        }
        // A fresh process cannot own a notification yet, so anything still in
        // the shade is left over from one that died.
        if (!MediaSessionState.hasMedia && MediaSessionCleanupService.instance == null) {
            MediaSessionCleanupService.cancel(activity.applicationContext)
        }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        requestNotificationPermission()
    }

    @Command
    fun initialize(invoke: Invoke) {
        requestNotificationPermission()
        invoke.resolve()
    }

    @Command
    fun updateState(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateStateArgs::class.java)
        val u = MediaUpdate()
        u.title = args.title
        u.artist = args.artist
        u.album = args.album
        u.artworkUrl = args.artworkUrl
        u.duration = args.duration
        u.position = args.position
        u.playbackSpeed = args.playbackSpeed
        u.isPlaying = args.isPlaying
        u.canPrev = args.canPrev
        u.canNext = args.canNext
        u.canSeek = args.canSeek
        u.shuffleEnabled = args.shuffleEnabled
        u.repeatMode = args.repeatMode
        MediaSessionState.update(u)
        invoke.resolve()
    }

    @Command
    fun updateTimeline(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateTimelineArgs::class.java)
        MediaSessionState.timeline(args.position, args.duration, args.playbackSpeed)
        invoke.resolve()
    }

    @Command
    fun clear(invoke: Invoke) {
        MediaSessionState.stop()
        invoke.resolve()
    }

    override fun onDestroy(activity: AppCompatActivity) {
        // Only the Activity is going away. The session, notification and
        // service carry on if playback does.
        if (activeInstance === this) activeInstance = null
        super.onDestroy(activity)
    }

    private fun requestNotificationPermission() {
        if (notificationPermissionRequested) return
        notificationPermissionRequested = true
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        if (activity.isFinishing || activity.isDestroyed) return
        if (ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            ActivityCompat.requestPermissions(
                activity,
                arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                NOTIFICATION_PERMISSION_REQUEST_CODE
            )
        }
    }

    private fun emit(payload: JSObject) {
        try {
            if (activity.isDestroyed) return
            activity.runOnUiThread {
                try {
                    trigger("media_action", payload)
                } catch (t: Throwable) {
                    Log.w(TAG, "media_action emit failed: ${t.message}")
                }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "media_action emit failed: ${t.message}")
        }
    }

    companion object {
        private const val TAG = "plugin/media-session"
        private const val NOTIFICATION_PERMISSION_REQUEST_CODE = 9402

        @Volatile
        internal var activeInstance: MediaSessionPlugin? = null

        internal fun isPlaybackActive(): Boolean {
            return try {
                Class.forName("app.bmdarklight.wave.audio.WaveExoPlayer")
                    .getMethod("isPlayingActive")
                    .invoke(null) as Boolean
            } catch (_: Throwable) {
                false
            }
        }

        /**
         * Dispatch a transport action.
         *
         * Goes to the host's native engine first (MediaNativeBridge), which
         * works with the WebView frozen or gone. JS only hears about it when
         * the native bridge is missing; doing both would double-fire
         * next/previous.
         */
        internal fun handleMediaAction(action: String) {
            Log.d(TAG, "handleMediaAction: \"$action\"")
            val nativeOk = try {
                Class.forName("app.bmdarklight.wave.MediaNativeBridge")
                    .getMethod("dispatch", String::class.java)
                    .invoke(null, action)
                true
            } catch (t: Throwable) {
                Log.w(TAG, "native media bridge unavailable: ${t.message}")
                false
            }
            if (nativeOk) return

            if (action == "dismiss") {
                MediaSessionState.stop()
            }
            val plugin = activeInstance ?: return
            val payload = JSObject()
            if (action.startsWith("seek:")) {
                val seconds = action.removePrefix("seek:").toDoubleOrNull() ?: return
                payload.put("action", "seek")
                payload.put("seekPosition", seconds)
            } else {
                payload.put("action", if (action == "dismiss") "pause" else action)
            }
            plugin.emit(payload)
        }
    }
}
