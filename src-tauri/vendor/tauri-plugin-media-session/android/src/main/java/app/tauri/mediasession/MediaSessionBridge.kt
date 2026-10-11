package app.tauri.mediasession

import android.content.Context
import org.json.JSONObject

/**
 * Entry points for the host app's native code.
 *
 * The host's audio engine drives the session directly through these (over
 * JNI), not through Tauri plugin commands. Plugin commands need a live Activity
 * to dispatch on, and the whole point of background playback is that the
 * Activity may be gone.
 */
object MediaSessionBridge {
    @JvmStatic
    fun init(context: Context) {
        MediaSessionState.init(context)
    }

    /** Apply a state change given as JSON with the [MediaUpdate] field names. */
    @JvmStatic
    fun update(json: String) {
        val o = JSONObject(json)
        val u = MediaUpdate()
        u.title = o.optStringOrNull("title")
        u.artist = o.optStringOrNull("artist")
        u.album = o.optStringOrNull("album")
        u.artworkUrl = o.optStringOrNull("artworkUrl")
        u.duration = o.optDoubleOrNull("duration")
        u.position = o.optDoubleOrNull("position")
        u.playbackSpeed = o.optDoubleOrNull("playbackSpeed")
        u.isPlaying = o.optBooleanOrNull("isPlaying")
        u.canPrev = o.optBooleanOrNull("canPrev")
        u.canNext = o.optBooleanOrNull("canNext")
        u.canSeek = o.optBooleanOrNull("canSeek")
        u.shuffleEnabled = o.optBooleanOrNull("shuffleEnabled")
        u.repeatMode = o.optStringOrNull("repeatMode")
        MediaSessionState.update(u)
    }

    /** Timeline-only update. Pass NaN for values that did not change. */
    @JvmStatic
    fun timeline(position: Double, duration: Double, speed: Double) {
        MediaSessionState.timeline(
            position.takeUnless { it.isNaN() },
            duration.takeUnless { it.isNaN() },
            speed.takeUnless { it.isNaN() },
        )
    }

    @JvmStatic
    fun clear() {
        MediaSessionState.stop()
    }

    private fun JSONObject.optStringOrNull(key: String): String? =
        if (has(key) && !isNull(key)) getString(key) else null

    private fun JSONObject.optDoubleOrNull(key: String): Double? =
        if (has(key) && !isNull(key)) getDouble(key).takeUnless { it.isNaN() } else null

    private fun JSONObject.optBooleanOrNull(key: String): Boolean? =
        if (has(key) && !isNull(key)) getBoolean(key) else null
}
