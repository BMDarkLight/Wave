package app.tauri.mediasession

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Build
import android.os.SystemClock
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.media.app.NotificationCompat as MediaNotificationCompat

/**
 * Process-wide media session + notification state.
 *
 * Lives outside the Activity-bound [MediaSessionPlugin] so background playback
 * keeps its transport notification after the UI Activity is destroyed.
 */
internal object MediaSessionState {
    private const val TAG = "plugin/media-session"
    private const val RC_PLAY = 1
    private const val RC_PAUSE = 2
    private const val RC_NEXT = 3
    private const val RC_PREV = 4
    private const val RC_SHUFFLE = 5
    private const val RC_REPEAT = 6
    private const val ACTION_SHUFFLE = "app.tauri.mediasession.SHUFFLE"
    private const val ACTION_REPEAT = "app.tauri.mediasession.REPEAT"

    var mediaSession: MediaSessionCompat? = null
    var cachedArtworkBitmap: Bitmap? = null

    var title: String = ""
    var artist: String = ""
    var album: String = ""
    var duration: Double = 0.0
    var position: Double = 0.0
    var playbackSpeed: Double = 1.0
    var isPlaying: Boolean = false
    var canPrev: Boolean = false
    var canNext: Boolean = false
    var canSeek: Boolean = true
    var shuffleEnabled: Boolean = false
    var repeatMode: String = "off"
    var lastStateUpdateRealtime: Long = 0L

    /** False after the user closes the app or playback is fully stopped. */
    @Volatile
    var sessionActive: Boolean = false

    fun hasActiveMedia(): Boolean =
        sessionActive && (title.isNotBlank() || artist.isNotBlank())

    fun appContext(): Context? = MediaSessionPlugin.hostApplicationContext()

    /** Called from Rust/JNI while the WebView may be frozen or Activity gone. */
    @JvmStatic
    fun refreshFromBackground(positionSec: Double, playing: Boolean) {
        if (!sessionActive) return
        position = positionSec
        isPlaying = playing
        val context = appContext() ?: return
        if (!hasActiveMedia()) return
        // Cold-start FGS only while playing — paused restore must not start it.
        if (playing || MediaSessionCleanupService.instance != null) {
            refreshForeground(context, advancePosition = false)
        }
    }

    /** Repost the foreground notification and keep the FGS alive. */
    fun refreshForeground(context: Context, advancePosition: Boolean = true) {
        if (!sessionActive || !hasActiveMedia()) return
        if (advancePosition) advancePositionToNow()

        val session = ensureSession(context) ?: return
        val metadata = buildMetadata()
        session.setMetadata(metadata)
        session.setPlaybackState(buildPlaybackState())
        session.isActive = true

        val notification = buildNotification(context, metadata, session) ?: return
        MediaSessionCleanupService.pendingNotification = notification
        MediaSessionCleanupService.start(context, notification)
    }

    fun clear(context: Context) {
        sessionActive = false
        mediaSession?.let { session ->
            try {
                session.isActive = false
                session.release()
            } catch (_: Throwable) {
            }
        }
        mediaSession = null
        title = ""
        artist = ""
        album = ""
        duration = 0.0
        position = 0.0
        playbackSpeed = 1.0
        isPlaying = false
        canPrev = false
        canNext = false
        canSeek = true
        shuffleEnabled = false
        repeatMode = "off"
        lastStateUpdateRealtime = 0L
        cachedArtworkBitmap = null
        NotificationManagerCompat.from(context).cancel(MediaSessionCleanupService.NOTIFICATION_ID)
    }

    private fun channelId(context: Context) = "${context.packageName}.media"
    private fun sessionTag(context: Context) = "${context.packageName}.MediaSession"

    private fun advancePositionToNow() {
        if (lastStateUpdateRealtime == 0L) return
        if (!isPlaying || playbackSpeed == 0.0) return
        val now = SystemClock.elapsedRealtime()
        val deltaSec = (now - lastStateUpdateRealtime) / 1000.0 * playbackSpeed
        if (deltaSec <= 0.0) return
        position += deltaSec
        if (duration > 0.0) {
            position = position.coerceAtMost(duration)
        }
    }

    private fun ensureSession(context: Context): MediaSessionCompat? {
        mediaSession?.let { return it }

        Log.d(TAG, "MediaSessionState.ensureSession")
        val session = MediaSessionCompat(context.applicationContext, sessionTag(context))
        session.setCallback(sessionCallback)
        session.setFlags(
            MediaSessionCompat.FLAG_HANDLES_MEDIA_BUTTONS or
                MediaSessionCompat.FLAG_HANDLES_TRANSPORT_CONTROLS
        )
        session.isActive = true

        val launchIntent = context.packageManager.getLaunchIntentForPackage(context.packageName)
        if (launchIntent != null) {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
            session.setSessionActivity(
                PendingIntent.getActivity(context.applicationContext, 0, launchIntent, flags)
            )
        }

        createNotificationChannel(context)
        mediaSession = session
        return session
    }

    private fun createNotificationChannel(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val id = channelId(context)
        if (manager.getNotificationChannel(id) != null) return
        val channel = NotificationChannel(id, "Media playback", NotificationManager.IMPORTANCE_LOW)
            .apply { description = "Media playback controls" }
        manager.createNotificationChannel(channel)
    }

    private fun buildMetadata(): MediaMetadataCompat {
        val builder = MediaMetadataCompat.Builder()
            .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
            .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, album)
        if (duration > 0.0) {
            builder.putLong(
                MediaMetadataCompat.METADATA_KEY_DURATION,
                (duration * 1000.0).toLong()
            )
        }
        cachedArtworkBitmap?.let {
            builder.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, it)
        }
        return builder.build()
    }

    private fun buildPlaybackState(): PlaybackStateCompat {
        val now = SystemClock.elapsedRealtime()
        lastStateUpdateRealtime = now
        val positionMs = (position * 1000.0).toLong()
        val state = if (isPlaying) PlaybackStateCompat.STATE_PLAYING
        else PlaybackStateCompat.STATE_PAUSED
        val speed = if (isPlaying) playbackSpeed.toFloat() else 0.0f

        var actions = PlaybackStateCompat.ACTION_PLAY or
            PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_PAUSE
        if (canSeek) actions = actions or PlaybackStateCompat.ACTION_SEEK_TO
        if (canPrev) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (canNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT

        return PlaybackStateCompat.Builder()
            .setActions(actions)
            .setState(state, positionMs, speed, now)
            .build()
    }

    private fun buildNotification(
        context: Context,
        metadata: MediaMetadataCompat,
        session: MediaSessionCompat,
    ): Notification? {
        val titleText = metadata.getString(MediaMetadataCompat.METADATA_KEY_TITLE)
            ?: context.applicationInfo.loadLabel(context.packageManager).toString()
        val artistText = metadata.getString(MediaMetadataCompat.METADATA_KEY_ARTIST)
        val albumText = metadata.getString(MediaMetadataCompat.METADATA_KEY_ALBUM)
        val subtitle = listOfNotNull(artistText, albumText).filter { it.isNotBlank() }.joinToString(" \u2014 ")
        val artwork = metadata.getBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART)

        val actions = mutableListOf<NotificationCompat.Action>()
        val compactIndices = mutableListOf<Int>()

        actions.add(
            NotificationCompat.Action(
                if (shuffleEnabled) R.drawable.ic_media_shuffle_on else R.drawable.ic_media_shuffle,
                "Shuffle",
                actionPendingIntent(context, "shuffle", RC_SHUFFLE)
            )
        )
        if (canPrev) {
            actions.add(
                NotificationCompat.Action(
                    android.R.drawable.ic_media_previous, "Previous",
                    actionPendingIntent(context, "previous", RC_PREV)
                )
            )
            compactIndices.add(actions.size - 1)
        }
        actions.add(
            if (isPlaying) {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_pause, "Pause",
                    actionPendingIntent(context, "pause", RC_PAUSE)
                )
            } else {
                NotificationCompat.Action(
                    android.R.drawable.ic_media_play, "Play",
                    actionPendingIntent(context, "play", RC_PLAY)
                )
            }
        )
        compactIndices.add(actions.size - 1)
        if (canNext) {
            actions.add(
                NotificationCompat.Action(
                    android.R.drawable.ic_media_next, "Next",
                    actionPendingIntent(context, "next", RC_NEXT)
                )
            )
            compactIndices.add(actions.size - 1)
        }
        actions.add(
            NotificationCompat.Action(
                when (repeatMode) {
                    "one" -> R.drawable.ic_media_repeat_one
                    "all" -> R.drawable.ic_media_repeat_on
                    else -> R.drawable.ic_media_repeat
                },
                "Repeat",
                actionPendingIntent(context, "repeat", RC_REPEAT)
            )
        )

        val builder = NotificationCompat.Builder(context, channelId(context))
            .setSmallIcon(smallIcon(context))
            .setContentTitle(titleText)
            .setContentText(subtitle)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setSilent(true)
            .setColorized(true)

        if (artwork != null) builder.setLargeIcon(artwork)

        val launchIntent = context.packageManager.getLaunchIntentForPackage(context.packageName)
        if (launchIntent != null) {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
            builder.setContentIntent(
                PendingIntent.getActivity(context.applicationContext, 0, launchIntent, flags)
            )
        }

        val style = MediaNotificationCompat.MediaStyle().setMediaSession(session.sessionToken)
        if (compactIndices.isNotEmpty()) {
            style.setShowActionsInCompactView(*compactIndices.toIntArray())
        }
        builder.setStyle(style)
        actions.forEach { builder.addAction(it) }
        return builder.build()
    }

    private fun actionPendingIntent(context: Context, action: String, requestCode: Int): PendingIntent {
        val intent = Intent(context, MediaActionReceiver::class.java)
            .putExtra(MediaActionReceiver.EXTRA_ACTION, action)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
        return PendingIntent.getBroadcast(context, requestCode, intent, flags)
    }

    private fun smallIcon(context: Context): Int {
        val res = context.resources
        val pkg = context.packageName
        var id = res.getIdentifier("ic_notification", "drawable", pkg)
        if (id != 0) return id
        id = res.getIdentifier("ic_notification", "mipmap", pkg)
        if (id != 0) return id
        id = res.getIdentifier("ic_launcher_foreground", "drawable", pkg)
        if (id != 0) return id
        return android.R.drawable.ic_media_play
    }

    /** Shared transport callback for both plugin- and service-owned sessions. */
    internal fun sessionCallback(): MediaSessionCompat.Callback = sessionCallback

    private val sessionCallback = object : MediaSessionCompat.Callback() {
        override fun onPlay() = MediaSessionPlugin.handleMediaAction("play")
        override fun onPause() = MediaSessionPlugin.handleMediaAction("pause")
        override fun onStop() = MediaSessionPlugin.handleMediaAction("stop")
        override fun onSkipToNext() = MediaSessionPlugin.handleMediaAction("next")
        override fun onSkipToPrevious() = MediaSessionPlugin.handleMediaAction("previous")
        override fun onSeekTo(pos: Long) =
            MediaSessionPlugin.handleMediaAction("seek:${pos / 1000.0}")
        override fun onCustomAction(action: String?, extras: android.os.Bundle?) {
            when (action) {
                ACTION_SHUFFLE -> MediaSessionPlugin.handleMediaAction("shuffle")
                ACTION_REPEAT -> MediaSessionPlugin.handleMediaAction("repeat")
            }
        }
        override fun onSetShuffleMode(shuffleMode: Int) =
            MediaSessionPlugin.handleMediaAction("shuffle")
        override fun onSetRepeatMode(repeatMode: Int) =
            MediaSessionPlugin.handleMediaAction("repeat")
    }
}
