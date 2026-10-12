package app.tauri.mediasession

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.drawable.BitmapDrawable
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Base64
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.media.app.NotificationCompat as MediaNotificationCompat
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors
import kotlin.math.abs

/** One state change. Null fields keep their previous value. */
class MediaUpdate {
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

/**
 * Process-wide owner of the media session, its notification and the playback
 * foreground service.
 *
 * Every mutation runs on the main thread. Callers on other threads (Rust over
 * JNI, the Tauri plugin, receivers) hand work over with [main], so updates land
 * in the order they were sent and never race each other.
 *
 * It lives outside the Activity-bound plugin so playback keeps its session and
 * notification after the UI has been destroyed.
 */
internal object MediaSessionState {
    private const val TAG = "plugin/media-session"
    internal const val NOTIFICATION_ID = 9401
    internal const val ACTION_SHUFFLE = "app.tauri.mediasession.SHUFFLE"
    internal const val ACTION_REPEAT = "app.tauri.mediasession.REPEAT"
    private const val MAX_ARTWORK_SIZE = 512

    // A track change arrives as several calls (metadata, play state,
    // shuffle/repeat). Wait a moment so they turn into one notification post;
    // Android drops updates from apps that post too often.
    private const val PUBLISH_DELAY_MS = 60L

    // How long to hold the notification back while new artwork decodes, so it
    // never shows the new title over the previous track's cover.
    private const val ARTWORK_WAIT_MS = 500L

    // Position reports this close to where the session already extrapolates
    // are dropped. Re-anchoring on every UI tick makes the system scrubber
    // jitter back and forth.
    private const val DRIFT_TOLERANCE_SEC = 1.0

    private const val RC_PLAY = 1
    private const val RC_PAUSE = 2
    private const val RC_NEXT = 3
    private const val RC_PREV = 4
    private const val RC_SHUFFLE = 5
    private const val RC_REPEAT = 6
    private const val RC_DISMISS = 7

    private val mainHandler = Handler(Looper.getMainLooper())
    private val artworkExecutor = Executors.newSingleThreadExecutor()

    @Volatile
    private var appContext: Context? = null

    private var session: MediaSessionCompat? = null

    private var title = ""
    private var artist = ""
    private var album = ""
    private var duration = 0.0
    private var position = 0.0
    private var playbackSpeed = 1.0
    private var isPlaying = false
    // Output held by a transient audio focus loss (a call, a navigation
    // prompt). The session shows paused, but playback will resume by itself.
    private var transientPause = false
    private var canPrev = false
    private var canNext = false
    private var canSeek = true
    private var shuffleEnabled = false
    private var repeatMode = "off"

    // elapsedRealtime at which [position] was true.
    private var anchorRealtime = 0L

    private var artworkUrl: String? = null
    private var artwork: Bitmap? = null
    private var artworkPendingSince = 0L
    private var fallbackArtwork: Bitmap? = null

    private var publishedMetadataKey: String? = null
    private var publishedNotificationKey: String? = null
    private var publishScheduled = false

    /** True while a track is loaded. Safe to read from any thread. */
    @Volatile
    var hasMedia = false
        private set

    /**
     * True while the service has to stay in the foreground: the track is
     * playing, or only held for a call and about to resume on its own. Safe to
     * read from any thread.
     *
     * A held track must keep the foreground. When the call ends ExoPlayer
     * resumes without any tap or button press, and Android refuses to start a
     * foreground service from the background without one. Media3 keeps the
     * foreground for the same reason (it goes by playWhenReady, which stays
     * true through a transient focus loss).
     */
    @Volatile
    var foregroundWanted = false
        private set

    fun init(context: Context) {
        if (appContext == null) {
            appContext = context.applicationContext
        }
    }

    fun context(): Context? = appContext

    fun main(block: () -> Unit) {
        mainHandler.post {
            try {
                block()
            } catch (t: Throwable) {
                Log.e(TAG, "media session update failed: ${t.message}", t)
                recordHostError("MediaSessionState", t)
            }
        }
    }

    fun update(update: MediaUpdate) = main { applyUpdate(update) }

    fun timeline(newPosition: Double?, newDuration: Double?, newSpeed: Double?) =
        main { applyTimeline(newPosition, newDuration, newSpeed) }

    fun stop() = main { stopInternal() }

    /** The player is holding output for a transient focus loss, or let go. */
    fun setTransientPause(held: Boolean) = main {
        if (transientPause == held) return@main
        transientPause = held
        foregroundWanted = hasMedia && (isPlaying || transientPause)
        if (hasMedia) schedulePublish()
    }

    /**
     * Take the session down synchronously. Main thread only; for callers that
     * are already on it and must not wait for a queued [stop].
     */
    fun stopNow() {
        stopInternal()
    }

    // ── State changes ───────────────────────────────────────────────────

    private fun applyUpdate(u: MediaUpdate) {
        val ctx = appContext ?: return

        // Without a fresh position, carry the old one forward to now so a
        // metadata or shuffle change does not pull the scrubber back.
        if (u.position == null) advanceToNow()

        u.title?.let { title = it.trim() }
        u.artist?.let { artist = it.trim() }
        u.album?.let { album = it.trim() }
        u.duration?.let { duration = it.coerceAtLeast(0.0) }
        u.position?.let { position = it }
        u.playbackSpeed?.let { if (it > 0.0) playbackSpeed = it }
        u.isPlaying?.let { isPlaying = it }
        u.canPrev?.let { canPrev = it }
        u.canNext?.let { canNext = it }
        u.canSeek?.let { canSeek = it }
        u.shuffleEnabled?.let { shuffleEnabled = it }
        u.repeatMode?.let { repeatMode = it }
        clampPosition()
        anchorRealtime = SystemClock.elapsedRealtime()
        u.artworkUrl?.let { setArtworkUrl(it) }

        hasMedia = title.isNotBlank() || artist.isNotBlank()
        foregroundWanted = hasMedia && (isPlaying || transientPause)
        if (!hasMedia) {
            stopInternal()
            return
        }

        val s = ensureSession(ctx)
        s.setPlaybackState(buildPlaybackState())
        s.setShuffleMode(
            if (shuffleEnabled) PlaybackStateCompat.SHUFFLE_MODE_ALL
            else PlaybackStateCompat.SHUFFLE_MODE_NONE
        )
        s.setRepeatMode(
            when (repeatMode) {
                "one" -> PlaybackStateCompat.REPEAT_MODE_ONE
                "all" -> PlaybackStateCompat.REPEAT_MODE_ALL
                else -> PlaybackStateCompat.REPEAT_MODE_NONE
            }
        )
        if (!s.isActive) s.isActive = true
        schedulePublish()
    }

    private fun applyTimeline(newPosition: Double?, newDuration: Double?, newSpeed: Double?) {
        if (!hasMedia) return
        val s = session ?: return
        var changed = false
        var durationChanged = false

        if (newDuration != null && newDuration > 0.0 && abs(newDuration - duration) > 0.5) {
            advanceToNow()
            duration = newDuration
            durationChanged = true
            changed = true
        }
        if (newSpeed != null && newSpeed > 0.0 && abs(newSpeed - playbackSpeed) > 0.001) {
            advanceToNow()
            playbackSpeed = newSpeed
            changed = true
        }
        if (newPosition != null && abs(newPosition - currentPosition()) >= DRIFT_TOLERANCE_SEC) {
            position = newPosition
            changed = true
        } else if (changed) {
            position = currentPosition()
        }
        if (!changed) return

        clampPosition()
        anchorRealtime = SystemClock.elapsedRealtime()
        s.setPlaybackState(buildPlaybackState())
        if (durationChanged) schedulePublish()
    }

    private fun stopInternal() {
        mainHandler.removeCallbacks(publishRunnable)
        publishScheduled = false
        hasMedia = false
        foregroundWanted = false
        transientPause = false

        // The session object is kept and only deactivated: releasing and
        // recreating it makes Bluetooth and car head units drop the player.
        session?.let { s ->
            try {
                s.setPlaybackState(
                    PlaybackStateCompat.Builder()
                        .setActions(0)
                        .setState(PlaybackStateCompat.STATE_NONE, 0L, 0f)
                        .build()
                )
                s.setMetadata(MediaMetadataCompat.Builder().build())
                s.isActive = false
            } catch (t: Throwable) {
                Log.w(TAG, "session deactivate failed: ${t.message}")
            }
        }

        title = ""; artist = ""; album = ""
        duration = 0.0; position = 0.0; playbackSpeed = 1.0; isPlaying = false
        canPrev = false; canNext = false; canSeek = true
        shuffleEnabled = false; repeatMode = "off"
        anchorRealtime = 0L
        artworkUrl = null
        artwork = null
        artworkPendingSince = 0L
        publishedMetadataKey = null
        publishedNotificationKey = null

        appContext?.let { MediaSessionCleanupService.stop(it) }
    }

    private fun currentPosition(): Double {
        if (!isPlaying || anchorRealtime == 0L) return position
        val elapsed = (SystemClock.elapsedRealtime() - anchorRealtime) / 1000.0
        val now = position + elapsed * playbackSpeed
        return if (duration > 0.0) now.coerceAtMost(duration) else now
    }

    private fun advanceToNow() {
        position = currentPosition()
        anchorRealtime = SystemClock.elapsedRealtime()
    }

    private fun clampPosition() {
        position = if (duration > 0.0) position.coerceIn(0.0, duration) else position.coerceAtLeast(0.0)
    }

    // ── Publishing ──────────────────────────────────────────────────────

    private fun schedulePublish() {
        if (publishScheduled) return
        publishScheduled = true
        mainHandler.postDelayed(publishRunnable, PUBLISH_DELAY_MS)
    }

    private val publishRunnable = object : Runnable {
        override fun run() {
            publishScheduled = false
            if (artworkPendingSince != 0L) {
                val waited = SystemClock.elapsedRealtime() - artworkPendingSince
                if (waited < ARTWORK_WAIT_MS) {
                    publishScheduled = true
                    mainHandler.postDelayed(this, ARTWORK_WAIT_MS - waited)
                    return
                }
            }
            try {
                publish()
            } catch (t: Throwable) {
                Log.e(TAG, "publish failed: ${t.message}", t)
                recordHostError("MediaSessionState.publish", t)
            }
        }
    }

    private fun publish() {
        val ctx = appContext ?: return
        if (!hasMedia) return
        val s = session ?: return

        val art = artwork ?: fallbackArtwork(ctx)
        val metadataKey = "$title\u0000$artist\u0000$album\u0000${durationMs()}\u0000${System.identityHashCode(art)}"
        if (metadataKey != publishedMetadataKey) {
            s.setMetadata(buildMetadata(art))
            publishedMetadataKey = metadataKey
        }

        val notificationKey = "$metadataKey\u0000$isPlaying\u0000$transientPause\u0000$canPrev\u0000$canNext\u0000$shuffleEnabled\u0000$repeatMode"
        if (notificationKey == publishedNotificationKey) return
        publishedNotificationKey = notificationKey
        MediaSessionCleanupService.publish(ctx, buildNotification(ctx, s, art), foregroundWanted)
    }

    private fun durationMs(): Long = (duration * 1000.0).toLong()

    private fun buildMetadata(art: Bitmap?): MediaMetadataCompat {
        val builder = MediaMetadataCompat.Builder()
            .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
            .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, album)
            .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_TITLE, title)
            .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_SUBTITLE, artist)
        if (duration > 0.0) {
            builder.putLong(MediaMetadataCompat.METADATA_KEY_DURATION, durationMs())
        }
        if (art != null) {
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, art)
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_ART, art)
        }
        return builder.build()
    }

    private fun buildPlaybackState(): PlaybackStateCompat {
        var actions = PlaybackStateCompat.ACTION_PLAY or
            PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_PAUSE or
            PlaybackStateCompat.ACTION_STOP or
            PlaybackStateCompat.ACTION_SET_SHUFFLE_MODE or
            PlaybackStateCompat.ACTION_SET_REPEAT_MODE
        if (canSeek) actions = actions or PlaybackStateCompat.ACTION_SEEK_TO
        if (canPrev) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (canNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT

        val state = if (isPlaying) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED
        val speed = if (isPlaying) playbackSpeed.toFloat() else 0f

        // Android 13+ builds the media controls from these actions, so the
        // shuffle and repeat buttons must be present on every state update or
        // they blink out of the player.
        return PlaybackStateCompat.Builder()
            .setActions(actions)
            .setState(state, (position * 1000.0).toLong(), speed, anchorRealtime)
            .addCustomAction(
                ACTION_SHUFFLE,
                if (shuffleEnabled) "Shuffle on" else "Shuffle off",
                if (shuffleEnabled) R.drawable.ic_media_shuffle_on else R.drawable.ic_media_shuffle
            )
            .addCustomAction(
                ACTION_REPEAT,
                when (repeatMode) {
                    "one" -> "Repeat one"
                    "all" -> "Repeat all"
                    else -> "Repeat off"
                },
                repeatIcon()
            )
            .build()
    }

    private fun repeatIcon(): Int = when (repeatMode) {
        "one" -> R.drawable.ic_media_repeat_one
        "all" -> R.drawable.ic_media_repeat_on
        else -> R.drawable.ic_media_repeat
    }

    private fun buildNotification(
        ctx: Context,
        s: MediaSessionCompat,
        art: Bitmap?,
    ): Notification {
        val actions = mutableListOf<NotificationCompat.Action>()
        val compact = mutableListOf<Int>()

        // Shuffle and repeat sit on the outer edges so previous / play / next
        // keep their places and are the three shown in the compact view.
        actions.add(
            NotificationCompat.Action(
                if (shuffleEnabled) R.drawable.ic_media_shuffle_on else R.drawable.ic_media_shuffle,
                "Shuffle",
                actionIntent(ctx, "shuffle", RC_SHUFFLE)
            )
        )
        if (canPrev) {
            actions.add(
                NotificationCompat.Action(
                    android.R.drawable.ic_media_previous, "Previous",
                    actionIntent(ctx, "previous", RC_PREV)
                )
            )
            compact.add(actions.size - 1)
        }
        actions.add(
            if (isPlaying) {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_pause, "Pause",
                    actionIntent(ctx, "pause", RC_PAUSE)
                )
            } else {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_play, "Play",
                    actionIntent(ctx, "play", RC_PLAY)
                )
            }
        )
        compact.add(actions.size - 1)
        if (canNext) {
            actions.add(
                NotificationCompat.Action(
                    android.R.drawable.ic_media_next, "Next",
                    actionIntent(ctx, "next", RC_NEXT)
                )
            )
            compact.add(actions.size - 1)
        }
        actions.add(
            NotificationCompat.Action(repeatIcon(), "Repeat", actionIntent(ctx, "repeat", RC_REPEAT))
        )

        val builder = NotificationCompat.Builder(ctx, channelId(ctx))
            .setSmallIcon(smallIcon(ctx))
            .setContentTitle(title.ifBlank { appLabel(ctx) })
            .setContentText(artist)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setSilent(true)
            .setColorized(true)
            // Dismissable once paused; swiping it away stops the session. A
            // track held for a call is not really paused, so keep it pinned.
            .setOngoing(isPlaying || transientPause)
            .setDeleteIntent(actionIntent(ctx, "dismiss", RC_DISMISS))

        if (art != null) builder.setLargeIcon(art)
        launchIntent(ctx)?.let { builder.setContentIntent(it) }

        builder.setStyle(
            MediaNotificationCompat.MediaStyle()
                .setMediaSession(s.sessionToken)
                .setShowActionsInCompactView(*compact.toIntArray())
        )
        actions.forEach { builder.addAction(it) }
        return builder.build()
    }

    // ── Session ─────────────────────────────────────────────────────────

    private fun ensureSession(ctx: Context): MediaSessionCompat {
        session?.let { return it }
        Log.d(TAG, "creating media session")
        val s = MediaSessionCompat(ctx, "${ctx.packageName}.MediaSession")
        s.setCallback(sessionCallback, mainHandler)
        @Suppress("DEPRECATION")
        s.setFlags(
            MediaSessionCompat.FLAG_HANDLES_MEDIA_BUTTONS or
                MediaSessionCompat.FLAG_HANDLES_TRANSPORT_CONTROLS
        )
        launchIntent(ctx)?.let { s.setSessionActivity(it) }
        ensureChannel(ctx)
        session = s
        return s
    }

    private val sessionCallback = object : MediaSessionCompat.Callback() {
        override fun onPlay() = MediaSessionPlugin.handleMediaAction("play")
        override fun onPause() = MediaSessionPlugin.handleMediaAction("pause")
        override fun onStop() = MediaSessionPlugin.handleMediaAction("stop")
        override fun onSkipToNext() = MediaSessionPlugin.handleMediaAction("next")
        override fun onSkipToPrevious() = MediaSessionPlugin.handleMediaAction("previous")
        override fun onSeekTo(pos: Long) {
            // Move the scrubber right away instead of waiting for the engine
            // to report back, so a drag in the shade does not snap back.
            if (hasMedia) {
                position = pos / 1000.0
                clampPosition()
                anchorRealtime = SystemClock.elapsedRealtime()
                session?.setPlaybackState(buildPlaybackState())
            }
            MediaSessionPlugin.handleMediaAction("seek:${pos / 1000.0}")
        }
        override fun onCustomAction(action: String?, extras: android.os.Bundle?) {
            when (action) {
                ACTION_SHUFFLE -> MediaSessionPlugin.handleMediaAction("shuffle")
                ACTION_REPEAT -> MediaSessionPlugin.handleMediaAction("repeat")
            }
        }
        override fun onSetShuffleMode(shuffleMode: Int) {
            val wanted = shuffleMode != PlaybackStateCompat.SHUFFLE_MODE_NONE
            if (wanted != shuffleEnabled) MediaSessionPlugin.handleMediaAction("shuffle")
        }
        override fun onSetRepeatMode(mode: Int) = MediaSessionPlugin.handleMediaAction("repeat")
    }

    internal fun channelId(ctx: Context) = "${ctx.packageName}.media"

    internal fun ensureChannel(ctx: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = ctx.getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(channelId(ctx)) != null) return
        val channel = NotificationChannel(
            channelId(ctx), "Media playback", NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Playback controls"
            setShowBadge(false)
        }
        manager.createNotificationChannel(channel)
    }

    /**
     * The host app's monochrome notification icon. Never the launcher
     * foreground: in a fresh Tauri project that drawable is the stock Android
     * robot, which is what the shade used to show.
     */
    internal fun smallIcon(ctx: Context): Int {
        val id = ctx.resources.getIdentifier("ic_notification", "drawable", ctx.packageName)
        return if (id != 0) id else android.R.drawable.ic_media_play
    }

    private fun appLabel(ctx: Context): String =
        ctx.applicationInfo.loadLabel(ctx.packageManager).toString()

    private fun launchIntent(ctx: Context): PendingIntent? {
        val intent = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName) ?: return null
        intent.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(
            ctx, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
    }

    private fun actionIntent(ctx: Context, action: String, requestCode: Int): PendingIntent {
        val intent = Intent(ctx, MediaActionReceiver::class.java)
            .putExtra(MediaActionReceiver.EXTRA_ACTION, action)
        return PendingIntent.getBroadcast(
            ctx, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
    }

    // ── Artwork ─────────────────────────────────────────────────────────

    private fun setArtworkUrl(url: String) {
        if (url.isEmpty()) {
            artworkUrl = null
            artwork = null
            artworkPendingSince = 0L
            return
        }
        if (url == artworkUrl) return
        artworkUrl = url
        artworkPendingSince = SystemClock.elapsedRealtime()
        val ctx = appContext
        artworkExecutor.execute {
            val bitmap = try {
                decodeArtwork(ctx, url)
            } catch (t: Throwable) {
                Log.w(TAG, "artwork decode failed: ${t.message}")
                null
            }
            main {
                if (artworkUrl != url) return@main
                artwork = bitmap
                artworkPendingSince = 0L
                if (hasMedia) schedulePublish()
            }
        }
    }

    private fun decodeArtwork(ctx: Context?, url: String): Bitmap? {
        val bytes: ByteArray = when {
            url.startsWith("data:") -> {
                val comma = url.indexOf(',')
                if (comma < 0) return null
                Base64.decode(url.substring(comma + 1), Base64.DEFAULT)
            }
            url.startsWith("file://") -> File(Uri.parse(url).path ?: return null).readBytes()
            url.startsWith("/") -> File(url).readBytes()
            url.startsWith("content://") -> {
                val resolver = ctx?.contentResolver ?: return null
                resolver.openInputStream(Uri.parse(url))?.use { it.readBytes() } ?: return null
            }
            url.startsWith("http://") || url.startsWith("https://") -> {
                val connection = URL(url).openConnection() as HttpURLConnection
                try {
                    connection.connectTimeout = 10_000
                    connection.readTimeout = 10_000
                    connection.instanceFollowRedirects = true
                    connection.inputStream.use { it.readBytes() }
                } finally {
                    connection.disconnect()
                }
            }
            else -> return null
        }
        return decodeSampled(bytes)
    }

    private fun decodeSampled(bytes: ByteArray): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= MAX_ARTWORK_SIZE &&
            bounds.outHeight / (sample * 2) >= MAX_ARTWORK_SIZE
        ) {
            sample *= 2
        }
        val opts = BitmapFactory.Options().apply { inSampleSize = sample }
        val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts) ?: return null
        return fitWithin(decoded, MAX_ARTWORK_SIZE)
    }

    private fun fitWithin(source: Bitmap, max: Int): Bitmap {
        if (source.width <= max && source.height <= max) return source
        val scale = minOf(max.toFloat() / source.width, max.toFloat() / source.height)
        return Bitmap.createScaledBitmap(
            source,
            (source.width * scale).toInt().coerceAtLeast(1),
            (source.height * scale).toInt().coerceAtLeast(1),
            true
        )
    }

    /** The app icon, shown when a track has no cover. */
    private fun fallbackArtwork(ctx: Context): Bitmap? {
        fallbackArtwork?.let { return it }
        return try {
            val drawable = ctx.packageManager.getApplicationIcon(ctx.applicationInfo)
            val bitmap = if (drawable is BitmapDrawable && drawable.bitmap != null) {
                drawable.bitmap
            } else {
                val size = 256
                Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888).also {
                    val canvas = Canvas(it)
                    drawable.setBounds(0, 0, size, size)
                    drawable.draw(canvas)
                }
            }
            fitWithin(bitmap, 256).also { fallbackArtwork = it }
        } catch (_: Throwable) {
            null
        }
    }

    internal fun recordHostError(where: String, error: Throwable) {
        val ctx = appContext ?: return
        try {
            Class.forName("app.bmdarklight.wave.CrashReporter")
                .getMethod("recordError", Context::class.java, String::class.java, Throwable::class.java)
                .invoke(null, ctx, where, error)
        } catch (_: Throwable) {
        }
    }
}
