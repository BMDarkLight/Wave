package app.tauri.mediasession

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.drawable.BitmapDrawable
import android.graphics.drawable.Drawable
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Base64
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.media.app.NotificationCompat as MediaNotificationCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

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

@TauriPlugin
class MediaSessionPlugin(private val activity: Activity) : Plugin(activity) {

    init {
        activeInstance = this
        Log.d(TAG, "init")
        try {
            Class.forName("app.bmdarklight.wave.CrashReporter")
                .getMethod("install", Context::class.java)
                .invoke(null, activity.applicationContext)
        } catch (_: Throwable) {
        }
        // Don't wipe an active media notification when the Activity/plugin
        // is recreated while background playback is still running.
        if (MediaSessionCleanupService.instance == null
            && !isPlaybackActive()
            && !MediaSessionState.sessionActive
        ) {
            cancelNotificationArtifacts(activity.applicationContext)
        }
    }

    // Native session & notification
    private var mediaSession: MediaSessionCompat? = null
    private var notificationManager: NotificationManagerCompat? = null

    // Artwork cache
    private var cachedArtworkUrl: String? = null
    private var cachedArtworkBitmap: Bitmap? = null
    private var fallbackArtworkBitmap: Bitmap? = null
    // Tracks the URL currently being downloaded — intentionally NOT cleared by artworkUrl=""
    // so that in-flight downloads survive the JS clear→url race condition.
    private var downloadingArtworkUrl: String? = null

    // Permission bookkeeping
    private var notificationPermissionRequested = false

    // Merge state — omitted fields in updateState keep their previous values
    private var currentTitle: String = ""
    private var currentArtist: String = ""
    private var currentAlbum: String = ""
    private var currentDuration: Double = 0.0
    private var currentPosition: Double = 0.0
    private var currentPlaybackSpeed: Double = 1.0
    private var currentIsPlaying: Boolean = false
    private var currentCanPrev: Boolean = false
    private var currentCanNext: Boolean = false
    private var currentCanSeek: Boolean = true
    private var currentShuffleEnabled: Boolean = false
    private var currentRepeatMode: String = "off"
    // ElapsedRealtime of the last PlaybackState setState call — used to
    // extrapolate position when updateState rebuilds without a fresh position
    // (shuffle/repeat/metadata), so the notification scrubber doesn't jump.
    private var lastStateUpdateRealtime: Long = 0L

    // Dynamic identifiers derived from the host app
    private val channelId: String by lazy { "${activity.packageName}.media" }
    private val sessionTag: String by lazy { "${activity.packageName}.MediaSession" }

    // ── Commands ────────────────────────────────────────────────────────

    @Command
    fun initialize(invoke: Invoke) {
        Log.d(TAG, "initialize: requesting permissions and creating session")
        requestNotificationPermission()
        ensureSession()
        invoke.resolve()
    }

    @Command
    fun updateState(invoke: Invoke) {
        try {
            updateStateInner(invoke)
        } catch (t: Throwable) {
            Log.e(TAG, "updateState crashed: ${t.message}", t)
            try {
                Class.forName("app.bmdarklight.wave.CrashReporter")
                    .getMethod(
                        "recordError",
                        Context::class.java,
                        String::class.java,
                        Throwable::class.java
                    )
                    .invoke(null, activity.applicationContext, "MediaSessionPlugin.updateState", t)
            } catch (_: Throwable) {
            }
            try {
                invoke.resolve()
            } catch (_: Throwable) {
            }
        }
    }

    private fun updateStateInner(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateStateArgs::class.java)

        // Log received fields (only the ones actually provided)
        val fields = mutableListOf<String>()
        args.title?.let { fields.add("title=\"$it\"") }
        args.artist?.let { fields.add("artist=\"$it\"") }
        args.album?.let { fields.add("album=\"$it\"") }
        args.artworkUrl?.let { fields.add("artworkUrl=\"$it\"") }
        args.duration?.let { fields.add("duration=${it}s") }
        args.position?.let { fields.add("position=${it}s") }
        args.playbackSpeed?.let { fields.add("playbackSpeed=${it}x") }
        args.isPlaying?.let { fields.add("isPlaying=$it") }
        args.canPrev?.let { fields.add("canPrev=$it") }
        args.canNext?.let { fields.add("canNext=$it") }
        args.canSeek?.let { fields.add("canSeek=$it") }
        args.shuffleEnabled?.let { fields.add("shuffleEnabled=$it") }
        args.repeatMode?.let { fields.add("repeatMode=$it") }
        Log.d(TAG, "updateState: received { ${fields.joinToString(", ")} }")

        val session = ensureSession() ?: run {
            Log.e(TAG, "updateState: media session unavailable")
            invoke.reject("media session unavailable")
            return
        }

        requestNotificationPermission()

        // ── Merge incoming args with stored state ───────────────────
        // When position is omitted (metadata / shuffle / repeat updates),
        // advance the stored position to "now" before re-anchoring so the
        // scrubber doesn't jump backward to a stale value.
        if (args.position == null) {
            advanceStoredPositionToNow()
        }

        args.title?.trim()?.let { currentTitle = it }
        args.artist?.trim()?.let { currentArtist = it }
        args.album?.trim()?.let { currentAlbum = it }
        args.duration?.let { currentDuration = it }
        args.position?.let { currentPosition = it }
        args.playbackSpeed?.let { currentPlaybackSpeed = it }
        args.isPlaying?.let { currentIsPlaying = it }
        args.canPrev?.let { currentCanPrev = it }
        args.canNext?.let { currentCanNext = it }
        args.canSeek?.let { currentCanSeek = it }
        args.shuffleEnabled?.let { currentShuffleEnabled = it }
        args.repeatMode?.let { currentRepeatMode = it }

        if (currentDuration > 0.0) {
            currentPosition = currentPosition.coerceIn(0.0, currentDuration)
        } else if (currentPosition < 0.0) {
            currentPosition = 0.0
        }

        val artworkUrl = args.artworkUrl

        if (artworkUrl != null) {
            if (artworkUrl.isEmpty()) {
                Log.d(TAG, "updateState: clearing artwork (URL)")
                applyFallbackArtwork(null)
                cachedArtworkUrl = null
                downloadingArtworkUrl = null
            } else if (artworkUrl != cachedArtworkUrl) {
                Log.d(TAG, "updateState: downloading artwork from URL")
                cachedArtworkUrl = artworkUrl
                downloadingArtworkUrl = artworkUrl
                downloadAndApplyArtwork(artworkUrl)
            }
        }

        // ── Metadata + playback state + notification ─────────────────
        val metadata = buildMetadata()
        session.setMetadata(metadata)
        session.setPlaybackState(buildPlaybackState())
        session.setShuffleMode(
            if (currentShuffleEnabled) PlaybackStateCompat.SHUFFLE_MODE_ALL
            else PlaybackStateCompat.SHUFFLE_MODE_NONE
        )
        session.setRepeatMode(
            when (currentRepeatMode) {
                "one" -> PlaybackStateCompat.REPEAT_MODE_ONE
                "all" -> PlaybackStateCompat.REPEAT_MODE_ALL
                else -> PlaybackStateCompat.REPEAT_MODE_NONE
            }
        )
        // Publish shared state before touching the FGS — start() ignores
        // calls while MediaSessionState.sessionActive is still false, and only
        // cold-starts the service while playback is active.
        syncSharedState()
        session.isActive = hasActiveMedia()
        if (hasActiveMedia()) {
            refreshTransportNotification(metadata)
        } else {
            dismissTransportNotification()
            MediaSessionCleanupService.stop()
            MediaSessionState.clear(activity.applicationContext)
        }

        Log.d(TAG, "updateState: applied -> \"$currentTitle\" by $currentArtist, " +
            "playing=$currentIsPlaying, pos=${currentPosition}s/${currentDuration}s, " +
            "artwork=${if (cachedArtworkBitmap != null) "yes" else "none"}")
        invoke.resolve()
    }

    @Command
    fun updateTimeline(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateTimelineArgs::class.java)

        val session = mediaSession ?: run {
            Log.e(TAG, "updateTimeline: session not initialized — call updateState first")
            invoke.reject("media session not initialized — call updateState first")
            return
        }

        args.position?.let { currentPosition = it }
        args.duration?.let { currentDuration = it }
        args.playbackSpeed?.let { currentPlaybackSpeed = it }

        session.setPlaybackState(buildPlaybackState())
        if (args.duration != null) session.setMetadata(buildMetadata())

        syncSharedState()
        // Only refresh the FGS while playing (or if it is already running).
        if (MediaSessionState.sessionActive && hasActiveMedia() && currentIsPlaying) {
            MediaSessionState.refreshForeground(activity.applicationContext, advancePosition = false)
        }

        invoke.resolve()
    }

    @Command
    fun clear(invoke: Invoke) {
        Log.d(TAG, "clear: releasing session and dismissing notification")
        releaseSession()
        invoke.resolve()
    }

    // ── Lifecycle ───────────────────────────────────────────────────────

    override fun onDestroy() {
        // Detach from the Activity without tearing down the FGS — the user may
        // have only sent the app to the background (Home). Swiping the task
        // away is handled in MediaSessionCleanupService.onTaskRemoved.
        Log.d(TAG, "onDestroy: detaching plugin")
        if (activeInstance === this) {
            activeInstance = null
        }
        super.onDestroy()
    }

    // ── State builders ──────────────────────────────────────────────────

    /**
     * Advance [currentPosition] by the wall-clock time since the last
     * PlaybackState update. Call before rebuilding state when the caller did
     * not supply a fresh position (e.g. shuffle/repeat/metadata-only updates).
     */
    private fun advanceStoredPositionToNow() {
        if (lastStateUpdateRealtime == 0L) return
        if (!currentIsPlaying || currentPlaybackSpeed == 0.0) return
        val now = SystemClock.elapsedRealtime()
        val deltaSec = (now - lastStateUpdateRealtime) / 1000.0 * currentPlaybackSpeed
        if (deltaSec <= 0.0) return
        currentPosition += deltaSec
        if (currentDuration > 0.0) {
            currentPosition = currentPosition.coerceAtMost(currentDuration)
        }
    }

    private fun buildPlaybackState(): PlaybackStateCompat {
        val now = SystemClock.elapsedRealtime()
        lastStateUpdateRealtime = now
        val positionMs = (currentPosition * 1000.0).toLong()
        val state = if (currentIsPlaying) PlaybackStateCompat.STATE_PLAYING
                    else PlaybackStateCompat.STATE_PAUSED
        val speed = if (currentIsPlaying) currentPlaybackSpeed.toFloat() else 0.0f

        val builder = PlaybackStateCompat.Builder()
            .setActions(buildAvailableActions())
            .setState(state, positionMs, speed, now)
            .addCustomAction(
                ACTION_SHUFFLE,
                if (currentShuffleEnabled) "Shuffle on" else "Shuffle off",
                if (currentShuffleEnabled) R.drawable.ic_media_shuffle_on
                else R.drawable.ic_media_shuffle
            )
            .addCustomAction(
                ACTION_REPEAT,
                when (currentRepeatMode) {
                    "one" -> "Repeat one"
                    "all" -> "Repeat all"
                    else -> "Repeat off"
                },
                when (currentRepeatMode) {
                    "one" -> R.drawable.ic_media_repeat_one
                    "all" -> R.drawable.ic_media_repeat_on
                    else -> R.drawable.ic_media_repeat
                }
            )
        return builder.build()
    }

    private fun buildAvailableActions(): Long {
        var actions = PlaybackStateCompat.ACTION_PLAY_PAUSE or
            PlaybackStateCompat.ACTION_PLAY or
            PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_STOP or
            PlaybackStateCompat.ACTION_SET_SHUFFLE_MODE or
            PlaybackStateCompat.ACTION_SET_REPEAT_MODE
        if (currentCanSeek) actions = actions or PlaybackStateCompat.ACTION_SEEK_TO
        if (currentCanPrev) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (currentCanNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT
        return actions
    }

    private fun buildMetadata(): MediaMetadataCompat {
        val builder = MediaMetadataCompat.Builder()
        if (currentTitle.isNotEmpty()) {
            builder.putString(MediaMetadataCompat.METADATA_KEY_TITLE, currentTitle)
        }
        if (currentArtist.isNotEmpty()) {
            builder.putString(MediaMetadataCompat.METADATA_KEY_ARTIST, currentArtist)
        }
        if (currentAlbum.isNotEmpty()) {
            builder.putString(MediaMetadataCompat.METADATA_KEY_ALBUM, currentAlbum)
        }
        val durationMs = (currentDuration * 1000.0).toLong()
        if (durationMs > 0) {
            builder.putLong(MediaMetadataCompat.METADATA_KEY_DURATION, durationMs)
        }
        cachedArtworkBitmap?.let {
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, it)
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_ART, it)
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_DISPLAY_ICON, it)
        }
        return builder.build()
    }

    // ── Internal helpers ────────────────────────────────────────────────

    private fun syncSharedState() {
        MediaSessionState.title = currentTitle
        MediaSessionState.artist = currentArtist
        MediaSessionState.album = currentAlbum
        MediaSessionState.duration = currentDuration
        MediaSessionState.position = currentPosition
        MediaSessionState.playbackSpeed = currentPlaybackSpeed
        MediaSessionState.isPlaying = currentIsPlaying
        MediaSessionState.canPrev = currentCanPrev
        MediaSessionState.canNext = currentCanNext
        MediaSessionState.canSeek = currentCanSeek
        MediaSessionState.shuffleEnabled = currentShuffleEnabled
        MediaSessionState.repeatMode = currentRepeatMode
        MediaSessionState.lastStateUpdateRealtime = lastStateUpdateRealtime
        MediaSessionState.mediaSession = mediaSession
        MediaSessionState.cachedArtworkBitmap = cachedArtworkBitmap
        MediaSessionState.sessionActive = currentTitle.isNotBlank() || currentArtist.isNotBlank()
    }

    private fun hasActiveMedia(): Boolean {
        return currentTitle.isNotBlank() || currentArtist.isNotBlank()
    }

    private fun releaseSession(nativeAction: String? = "pause") {
        if (nativeAction != null) {
            try {
                handleMediaAction(nativeAction)
            } catch (_: Throwable) {
            }
        }
        mediaSession?.let { session ->
            try {
                val stoppedState = PlaybackStateCompat.Builder()
                    .setActions(0)
                    .setState(PlaybackStateCompat.STATE_NONE, 0L, 0f)
                    .build()
                session.setPlaybackState(stoppedState)
                session.setMetadata(MediaMetadataCompat.Builder().build())
            } catch (_: Throwable) {
            }
            session.isActive = false
            session.release()
        }
        mediaSession = null
        notificationManager?.cancel(MediaSessionCleanupService.NOTIFICATION_ID)
        cancelNotificationArtifacts(activity.applicationContext)
        MediaSessionCleanupService.stop()
        MediaSessionState.clear(activity.applicationContext)

        currentTitle = ""; currentArtist = ""; currentAlbum = ""
        currentDuration = 0.0; currentPosition = 0.0; currentPlaybackSpeed = 1.0
        currentIsPlaying = false; currentCanPrev = false; currentCanNext = false; currentCanSeek = true
        currentShuffleEnabled = false; currentRepeatMode = "off"
        lastStateUpdateRealtime = 0L

        recycleCachedArtworkBitmap()
        cachedArtworkUrl = null
        downloadingArtworkUrl = null
    }

    private fun requestNotificationPermission() {
        if (notificationPermissionRequested) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
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
        notificationPermissionRequested = true
    }

    @Suppress("DEPRECATION")
    private fun ensureSession(): MediaSessionCompat? {
        MediaSessionState.mediaSession?.let {
            mediaSession = it
            return it
        }
        if (mediaSession != null) return mediaSession

        Log.d(TAG, "ensureSession: creating new session (tag=$sessionTag, channel=$channelId)")
        // Application context outlives the Activity so the session can stay
        // active while the UI is destroyed/recreated in the background.
        val session = MediaSessionCompat(activity.applicationContext, sessionTag)
        session.setCallback(MediaSessionState.sessionCallback())
        session.setFlags(
            MediaSessionCompat.FLAG_HANDLES_MEDIA_BUTTONS or
                MediaSessionCompat.FLAG_HANDLES_TRANSPORT_CONTROLS
        )
        session.isActive = true

        val launchIntent = activity.packageManager.getLaunchIntentForPackage(activity.packageName)
        if (launchIntent != null) {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
            session.setSessionActivity(PendingIntent.getActivity(activity, 0, launchIntent, flags))
        }

        mediaSession = session
        MediaSessionState.mediaSession = session
        notificationManager = NotificationManagerCompat.from(activity)
        createNotificationChannel()
        return mediaSession
    }

    // ── Artwork download (native HTTP, no CORS) ─────────────────────────

    private fun downloadAndApplyArtwork(url: String) {
        Thread {
            try {
                val bitmap = fetchBitmapFromImageUrl(url)
                if (bitmap != null) {
                    Log.d(TAG, "downloadArtwork: decoded ${bitmap.width}x${bitmap.height} from $url")
                    activity.runOnUiThread {
                        // Apply if this is still the most recently requested URL.
                        // Using downloadingArtworkUrl (not cachedArtworkUrl) so the result
                        // survives an artworkUrl="" clear that arrived after the download started.
                        if (downloadingArtworkUrl == url) {
                            downloadingArtworkUrl = null
                            recycleCachedArtworkBitmap()
                            cachedArtworkBitmap = bitmap
                            cachedArtworkUrl = url
                            val session = mediaSession ?: return@runOnUiThread
                            val metadata = buildMetadata()
                            session.setMetadata(metadata)
                            refreshTransportNotification(metadata)
                        } else {
                            downloadingArtworkUrl = null
                            bitmap.recycle()
                        }
                    }
                } else {
                    Log.w(TAG, "downloadArtwork: decode returned null for $url")
                    applyFallbackArtwork(url)
                }
            } catch (e: Exception) {
                Log.w(TAG, "downloadArtwork: failed for $url: ${e.message}")
                applyFallbackArtwork(url)
            }
        }.start()
    }

    private fun fetchBitmapFromImageUrl(url: String): Bitmap? {
        if (url.startsWith("data:")) {
            val comma = url.indexOf(',')
            if (comma < 0) return null
            val bytes = Base64.decode(url.substring(comma + 1), Base64.DEFAULT)
            return decodeSampledBitmap(bytes, MAX_ARTWORK_SIZE)
        }

        if (url.startsWith("file://")) {
            val path = Uri.parse(url).path ?: return null
            val bytes = File(path).readBytes()
            return decodeSampledBitmap(bytes, MAX_ARTWORK_SIZE)
        }

        var connection: HttpURLConnection? = null
        return try {
            connection = URL(url).openConnection() as HttpURLConnection
            connection.connectTimeout = 10000
            connection.readTimeout = 10000
            connection.instanceFollowRedirects = true
            connection.connect()
            val contentType = connection.contentType?.lowercase() ?: ""
            if (contentType.startsWith("video/")) {
                null
            } else {
                val bytes = connection.inputStream.use { it.readBytes() }
                decodeSampledBitmap(bytes, MAX_ARTWORK_SIZE)
            }
        } catch (_: Exception) {
            null
        } finally {
            connection?.disconnect()
        }
    }

    private fun applyFallbackArtwork(expectedUrl: String?) {
        activity.runOnUiThread {
            if (expectedUrl != null && downloadingArtworkUrl != expectedUrl) return@runOnUiThread
            downloadingArtworkUrl = null
            cachedArtworkUrl = null
            recycleCachedArtworkBitmap()
            cachedArtworkBitmap = getFallbackArtworkBitmap()

            val session = mediaSession ?: return@runOnUiThread
            val metadata = buildMetadata()
            session.setMetadata(metadata)
            refreshTransportNotification(metadata)
        }
    }

    private fun refreshTransportNotification(metadata: MediaMetadataCompat? = null) {
        if (!hasActiveMedia()) return
        val meta = metadata ?: buildMetadata()
        val notification = updateNotification(meta)
        if (notification != null) MediaSessionCleanupService.start(activity, notification)
    }

    private fun dismissTransportNotification() {
        notificationManager?.cancel(MediaSessionCleanupService.NOTIFICATION_ID)
    }

    private fun recycleCachedArtworkBitmap() {
        val bitmap = cachedArtworkBitmap
        if (bitmap != null && bitmap !== fallbackArtworkBitmap) {
            bitmap.recycle()
        }
        cachedArtworkBitmap = null
    }

    private fun getFallbackArtworkBitmap(): Bitmap? {
        if (fallbackArtworkBitmap != null) return fallbackArtworkBitmap
        return try {
            val drawable = activity.packageManager.getApplicationIcon(activity.applicationInfo)
            val bitmap = drawableToBitmap(drawable)
            val scaled = scaleBitmapToMax(bitmap, MAX_ARTWORK_SIZE)
            fallbackArtworkBitmap = scaled
            fallbackArtworkBitmap
        } catch (_: Throwable) {
            null
        }
    }

    private fun drawableToBitmap(drawable: Drawable): Bitmap {
        if (drawable is BitmapDrawable && drawable.bitmap != null) {
            return drawable.bitmap
        }
        val width = if (drawable.intrinsicWidth > 0) drawable.intrinsicWidth else MAX_ARTWORK_SIZE
        val height = if (drawable.intrinsicHeight > 0) drawable.intrinsicHeight else MAX_ARTWORK_SIZE
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        drawable.setBounds(0, 0, canvas.width, canvas.height)
        drawable.draw(canvas)
        return bitmap
    }

    private fun scaleBitmapToMax(source: Bitmap, maxSize: Int): Bitmap {
        val width = source.width
        val height = source.height
        if (width <= maxSize && height <= maxSize) return source

        val widthRatio = maxSize.toFloat() / width.toFloat()
        val heightRatio = maxSize.toFloat() / height.toFloat()
        val scale = if (widthRatio < heightRatio) widthRatio else heightRatio
        val outWidth = (width * scale).toInt().coerceAtLeast(1)
        val outHeight = (height * scale).toInt().coerceAtLeast(1)
        val scaled = Bitmap.createScaledBitmap(source, outWidth, outHeight, true)
        if (scaled !== source) source.recycle()
        return scaled
    }

    // ── Notification action PendingIntents ───────────────────────────────

    private fun buildActionPendingIntent(action: String, requestCode: Int): PendingIntent {
        val intent = Intent(activity, MediaActionReceiver::class.java)
            .putExtra(MediaActionReceiver.EXTRA_ACTION, action)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
        return PendingIntent.getBroadcast(activity, requestCode, intent, flags)
    }

    private fun buildShuffleAction(): NotificationCompat.Action {
        val icon = if (currentShuffleEnabled) R.drawable.ic_media_shuffle_on else R.drawable.ic_media_shuffle
        return NotificationCompat.Action(icon, "Shuffle", buildActionPendingIntent("shuffle", RC_SHUFFLE))
    }

    private fun buildRepeatAction(): NotificationCompat.Action {
        val icon = when (currentRepeatMode) {
            "one" -> R.drawable.ic_media_repeat_one
            "all" -> R.drawable.ic_media_repeat_on
            else -> R.drawable.ic_media_repeat
        }
        return NotificationCompat.Action(icon, "Repeat", buildActionPendingIntent("repeat", RC_REPEAT))
    }

    private fun getSmallIcon(): Int {
        val res = activity.resources
        val pkg = activity.packageName
        var id = res.getIdentifier("ic_notification", "drawable", pkg)
        if (id != 0) return id
        id = res.getIdentifier("ic_notification", "mipmap", pkg)
        if (id != 0) return id
        id = res.getIdentifier("ic_launcher_foreground", "drawable", pkg)
        if (id != 0) return id
        return android.R.drawable.ic_media_play
    }

    // ── Notification ────────────────────────────────────────────────────

    private fun updateNotification(metadata: MediaMetadataCompat): Notification? {
        val session = mediaSession ?: return null
        val manager = notificationManager ?: return null

        val title = metadata.getString(MediaMetadataCompat.METADATA_KEY_TITLE)
            ?: activity.applicationInfo.loadLabel(activity.packageManager).toString()
        val artist = metadata.getString(MediaMetadataCompat.METADATA_KEY_ARTIST)
        val album = metadata.getString(MediaMetadataCompat.METADATA_KEY_ALBUM)
        val subtitle = listOfNotNull(artist, album).filter { it.isNotBlank() }.joinToString(" \u2014 ")
        val artwork = metadata.getBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART)

        // Shuffle/repeat sit on the outer edges so the primary transport
        // controls (previous/play/next) keep their positions and are the
        // ones picked for the 3-icon compact view.
        val actions = mutableListOf<NotificationCompat.Action>()
        val compactIndices = mutableListOf<Int>()

        actions.add(buildShuffleAction())

        if (currentCanPrev) {
            actions.add(NotificationCompat.Action(
                android.R.drawable.ic_media_previous, "Previous",
                buildActionPendingIntent("previous", RC_PREV)
            ))
            compactIndices.add(actions.size - 1)
        }
        actions.add(
            if (currentIsPlaying) {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_pause, "Pause",
                    buildActionPendingIntent("pause", RC_PAUSE)
                )
            } else {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_play, "Play",
                    buildActionPendingIntent("play", RC_PLAY)
                )
            }
        )
        compactIndices.add(actions.size - 1)
        if (currentCanNext) {
            actions.add(NotificationCompat.Action(
                android.R.drawable.ic_media_next, "Next",
                buildActionPendingIntent("next", RC_NEXT)
            ))
            compactIndices.add(actions.size - 1)
        }

        actions.add(buildRepeatAction())

        val builder = NotificationCompat.Builder(activity, channelId)
            .setSmallIcon(getSmallIcon())
            .setContentTitle(title)
            .setContentText(subtitle)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setSilent(true)
            .setColorized(true)

        if (artwork != null) builder.setLargeIcon(artwork)

        val launchIntent = activity.packageManager.getLaunchIntentForPackage(activity.packageName)
        if (launchIntent != null) {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
            builder.setContentIntent(PendingIntent.getActivity(activity, 0, launchIntent, flags))
        }

        val style = MediaNotificationCompat.MediaStyle()
            .setMediaSession(session.sessionToken)
        if (compactIndices.isNotEmpty()) {
            style.setShowActionsInCompactView(*compactIndices.toIntArray())
        }
        builder.setStyle(style)
        actions.forEach { builder.addAction(it) }

        val notification = builder.build()
        manager.notify(MediaSessionCleanupService.NOTIFICATION_ID, notification)
        return notification
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = activity.getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(channelId) != null) return
        val channel = NotificationChannel(
            channelId, "Media playback", NotificationManager.IMPORTANCE_LOW
        ).apply { description = "Media playback controls" }
        manager.createNotificationChannel(channel)
    }

    // ── Events → JavaScript ─────────────────────────────────────────────

    internal fun hostApplicationContext(): Context = activity.applicationContext

    internal fun emitAction(action: String) {
        Log.d(TAG, "emit: action=\"$action\"")
        val payload = JSObject()
        payload.put("action", action)
        try {
            if (activity.isDestroyed) {
                Log.w(TAG, "emitAction: activity destroyed, skipping JS for \"$action\"")
                return
            }
            activity.runOnUiThread {
                try {
                    trigger("media_action", payload)
                } catch (t: Throwable) {
                    Log.w(TAG, "emitAction trigger failed: ${t.message}")
                }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "emitAction failed: ${t.message}")
        }
    }

    // ── Bitmap helpers ──────────────────────────────────────────────────

    private fun decodeSampledBitmap(bytes: ByteArray, maxSize: Int): Bitmap? {
        val opts = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts)
        opts.inSampleSize = calculateInSampleSize(opts.outWidth, opts.outHeight, maxSize)
        opts.inJustDecodeBounds = false
        return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts)
    }

    private fun calculateInSampleSize(width: Int, height: Int, maxSize: Int): Int {
        var sample = 1
        var w = width; var h = height
        while (w / 2 >= maxSize && h / 2 >= maxSize) { sample *= 2; w /= 2; h /= 2 }
        return sample
    }

    companion object {
        private const val TAG = "plugin/media-session"
        private const val NOTIFICATION_PERMISSION_REQUEST_CODE = 9402
        private const val MAX_ARTWORK_SIZE = 1024
        private const val RC_PLAY = 1
        private const val RC_PAUSE = 2
        private const val RC_NEXT = 3
        private const val RC_PREV = 4
        private const val RC_SHUFFLE = 5
        private const val RC_REPEAT = 6
        private const val ACTION_SHUFFLE = "app.tauri.mediasession.SHUFFLE"
        private const val ACTION_REPEAT = "app.tauri.mediasession.REPEAT"

        @Volatile
        internal var activeInstance: MediaSessionPlugin? = null

        internal fun hostApplicationContext(): Context? {
            return activeInstance?.hostApplicationContext()
                ?: MediaSessionCleanupService.instance?.applicationContext
        }

        private fun cancelNotificationArtifacts(context: Context, hard: Boolean = false) {
            try {
                NotificationManagerCompat.from(context).cancel(MediaSessionCleanupService.NOTIFICATION_ID)
            } catch (_: Throwable) {
            }

            try {
                val manager = context.getSystemService(NotificationManager::class.java) ?: return
                manager.cancel(MediaSessionCleanupService.NOTIFICATION_ID)

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    val expectedChannel = "${context.packageName}.media"
                    for (status in manager.activeNotifications) {
                        val notification = status.notification
                        val sameId = status.id == MediaSessionCleanupService.NOTIFICATION_ID
                        val sameCategory = notification.category == NotificationCompat.CATEGORY_TRANSPORT
                        val sameChannel = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                            notification.channelId == expectedChannel
                        } else {
                            false
                        }
                        if (sameId || sameCategory || sameChannel) {
                            manager.cancel(status.tag, status.id)
                        }
                    }
                }

                if (hard && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    val expectedChannel = "${context.packageName}.media"
                    if (manager.getNotificationChannel(expectedChannel) != null) {
                        manager.deleteNotificationChannel(expectedChannel)
                    }
                }
            } catch (_: Throwable) {
            }
        }

        internal fun isPlaybackActive(): Boolean {
            return try {
                val cls = Class.forName("app.bmdarklight.wave.audio.WaveExoPlayer")
                cls.getMethod("isPlayingActive").invoke(null) as Boolean
            } catch (_: Throwable) {
                false
            }
        }

        /** Called from Rust/JNI while the WebView may be frozen or Activity gone. */
        @JvmStatic
        fun syncSessionFromBackground(positionSec: Double, playing: Boolean) {
            MediaSessionState.refreshFromBackground(positionSec, playing)
        }

        internal fun forceCleanup(context: Context) {
            MediaSessionCleanupService.instance?.cancelKeepalive()
            MediaSessionCleanupService.pendingNotification = null
            // Deactivate first so keepalive / Rust ticks cannot repost while stopping.
            MediaSessionState.clear(context)
            handleMediaAction("stop")
            val plugin = activeInstance
            if (plugin != null) {
                plugin.releaseSession(nativeAction = null)
            } else {
                MediaSessionCleanupService.stop()
                cancelNotificationArtifacts(context, hard = true)
            }
        }

        /** Soft notification cleanup without releasing an active playback session. */
        internal fun cancelNotificationArtifactsOnly(context: Context) {
            cancelNotificationArtifacts(context, hard = false)
        }

        /**
         * Dispatch a transport action.
         *
         * Prefers the host app's [MediaNativeBridge] (Rust audio engine) so
         * controls work while the WebView is frozen. Falls back to JS emit
         * only when the native bridge is unavailable. Never does both for
         * transport actions — that would double-fire next/previous.
         */
        internal fun handleMediaAction(action: String) {
            Log.d(TAG, "handleMediaAction: \"$action\"")
            var nativeOk = false
            try {
                val cls = Class.forName("app.bmdarklight.wave.MediaNativeBridge")
                val method = cls.getMethod("dispatch", String::class.java)
                method.invoke(null, action)
                nativeOk = true
            } catch (t: Throwable) {
                Log.w(TAG, "native media bridge unavailable: ${t.message}")
            }

            if (nativeOk) {
                return
            }

            val plugin = activeInstance
            if (plugin == null) {
                Log.w(TAG, "handleMediaAction: no plugin instance for JS fallback \"$action\"")
                return
            }

            // Seek payloads are "seek:<seconds>" for the native bridge; JS
            // expects a structured event.
            if (action.startsWith("seek:")) {
                val seconds = action.removePrefix("seek:").toDoubleOrNull() ?: return
                val payload = JSObject()
                payload.put("action", "seek")
                payload.put("seekPosition", seconds)
                try {
                    if (!plugin.activity.isDestroyed) {
                        plugin.activity.runOnUiThread {
                            try {
                                plugin.trigger("media_action", payload)
                            } catch (t: Throwable) {
                                Log.w(TAG, "seek JS emit failed: ${t.message}")
                            }
                        }
                    }
                } catch (t: Throwable) {
                    Log.w(TAG, "seek JS emit failed: ${t.message}")
                }
                return
            }

            plugin.emitAction(action)
        }
    }
}
