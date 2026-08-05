package app.bmdarklight.wave.audio;

import android.content.Context;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import androidx.media3.common.AudioAttributes;
import androidx.media3.common.C;
import androidx.media3.common.MediaItem;
import androidx.media3.common.PlaybackException;
import androidx.media3.common.Player;
import androidx.media3.exoplayer.ExoPlayer;

/**
 * Lightweight ExoPlayer holder for Wave.
 *
 * Owns decode/output only. MediaSession notifications stay with
 * tauri-plugin-media-session + MediaNativeBridge.
 *
 * Play/pause/stop/uri load run on the main looper and block the JNI caller
 * briefly so transport state is consistent when Rust returns. Position is
 * cached from player listeners and a 250ms ticker while playing.
 */
public final class WaveExoPlayer {
    private static final String TAG = "WaveExoPlayer";
    /** Allow slower main-thread work during library sync / WebView load. */
    private static final long BLOCKING_TIMEOUT_MS = 1500L;
    private static final long POSITION_TICK_MS = 250L;

    private static volatile WaveExoPlayer INSTANCE;

    private final Context appContext;
    private final Handler mainHandler;
    private ExoPlayer player;
    private volatile boolean ended;
    private volatile boolean playingCached;
    private volatile long positionMsCached;
    private volatile long durationMsCached;
    private final Runnable positionTicker = new Runnable() {
        @Override
        public void run() {
            if (player != null && player.isPlaying()) {
                refreshCacheFromPlayer();
                mainHandler.postDelayed(this, POSITION_TICK_MS);
            }
        }
    };

    private WaveExoPlayer(Context context) {
        this.appContext = context.getApplicationContext();
        this.mainHandler = new Handler(Looper.getMainLooper());
        runOnMainBlocking(this::initPlayer);
    }

    public static WaveExoPlayer getOrCreate(Context context) {
        WaveExoPlayer existing = INSTANCE;
        if (existing != null) {
            return existing;
        }
        synchronized (WaveExoPlayer.class) {
            if (INSTANCE == null) {
                INSTANCE = new WaveExoPlayer(context);
            }
            return INSTANCE;
        }
    }

    private void initPlayer() {
        if (player != null) {
            return;
        }
        AudioAttributes attrs = new AudioAttributes.Builder()
                .setUsage(C.USAGE_MEDIA)
                .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
                .build();

        // ExoPlayer owns audio focus. MediaSessionCleanupService must NOT also
        // request AUDIOFOCUS_GAIN — that steals focus back from Exo right after
        // play starts and leaves playback paused until the user retries.
        player = new ExoPlayer.Builder(appContext)
                .setAudioAttributes(attrs, /* handleAudioFocus= */ true)
                .setHandleAudioBecomingNoisy(true)
                .build();

        player.addListener(new Player.Listener() {
            @Override
            public void onPlaybackStateChanged(int playbackState) {
                ended = playbackState == Player.STATE_ENDED;
                refreshCacheFromPlayer();
                syncPositionTicker();
            }

            @Override
            public void onIsPlayingChanged(boolean isPlaying) {
                playingCached = isPlaying;
                refreshCacheFromPlayer();
                syncPositionTicker();
            }

            @Override
            public void onPlayerError(PlaybackException error) {
                Log.e(TAG, "Playback error: " + error.getMessage());
                ended = true;
                playingCached = false;
                stopPositionTicker();
            }
        });
    }

    private void refreshCacheFromPlayer() {
        if (player == null) {
            return;
        }
        playingCached = player.isPlaying();
        positionMsCached = player.getCurrentPosition();
        long d = player.getDuration();
        durationMsCached = d == C.TIME_UNSET ? 0L : d;
    }

    private void syncPositionTicker() {
        stopPositionTicker();
        if (player != null && player.isPlaying()) {
            mainHandler.postDelayed(positionTicker, POSITION_TICK_MS);
        }
    }

    private void stopPositionTicker() {
        mainHandler.removeCallbacks(positionTicker);
    }

    public void playUri(String uriString) {
        if (uriString == null || uriString.isEmpty()) {
            throw new IllegalArgumentException("uri is empty");
        }
        runOnMainBlocking(() -> {
            initPlayer();
            ended = false;
            Uri uri = Uri.parse(normalizeUri(uriString));
            player.setMediaItem(MediaItem.fromUri(uri));
            player.prepare();
            player.play();
            playingCached = true;
            refreshCacheFromPlayer();
            syncPositionTicker();
        });
    }

    /** Prepare a URI and seek without starting playback (session restore). */
    public void prepareUriAt(String uriString, long positionMs) {
        if (uriString == null || uriString.isEmpty()) {
            throw new IllegalArgumentException("uri is empty");
        }
        runOnMainBlocking(() -> {
            initPlayer();
            ended = false;
            Uri uri = Uri.parse(normalizeUri(uriString));
            player.setMediaItem(MediaItem.fromUri(uri));
            player.prepare();
            long clamped = Math.max(0L, positionMs);
            player.seekTo(clamped);
            player.pause();
            playingCached = false;
            positionMsCached = clamped;
            refreshCacheFromPlayer();
            stopPositionTicker();
        });
    }

    public void play() {
        // Blocking so resume() doesn't return before Exo has actually started
        // (async fire-and-forget raced the UI poll and looked "stuck").
        runOnMainBlocking(() -> {
            if (player != null) {
                ended = false;
                player.play();
                playingCached = true;
                refreshCacheFromPlayer();
                syncPositionTicker();
            }
        });
    }

    public void pause() {
        runOnMainBlocking(() -> {
            if (player != null) {
                refreshCacheFromPlayer();
                player.pause();
                playingCached = false;
                stopPositionTicker();
            }
        });
    }

    public void stop() {
        runOnMainBlocking(() -> {
            if (player != null) {
                stopPositionTicker();
                player.stop();
                player.clearMediaItems();
                ended = false;
                playingCached = false;
                positionMsCached = 0L;
                durationMsCached = 0L;
            }
        });
    }

    public void seekTo(long positionMs) {
        runOnMainAsync(() -> {
            if (player != null) {
                ended = false;
                long clamped = Math.max(0L, positionMs);
                player.seekTo(clamped);
                positionMsCached = clamped;
            }
        });
    }

    public void setVolume(float volume) {
        float clamped = Math.max(0f, Math.min(1f, volume));
        runOnMainAsync(() -> {
            if (player != null) {
                player.setVolume(clamped);
            }
        });
    }

    public long getCurrentPosition() {
        if (Looper.myLooper() == Looper.getMainLooper() && player != null) {
            positionMsCached = player.getCurrentPosition();
        }
        return positionMsCached;
    }

    public long getDuration() {
        if (Looper.myLooper() == Looper.getMainLooper() && player != null) {
            long d = player.getDuration();
            durationMsCached = d == C.TIME_UNSET ? 0L : d;
        }
        return durationMsCached;
    }

    public boolean isPlaying() {
        if (Looper.myLooper() == Looper.getMainLooper() && player != null) {
            playingCached = player.isPlaying();
        }
        return playingCached;
    }

    public boolean isEnded() {
        return ended;
    }

    private static String normalizeUri(String uriString) {
        if (uriString.startsWith("content://")
                || uriString.startsWith("file://")
                || uriString.startsWith("http://")
                || uriString.startsWith("https://")) {
            return uriString;
        }
        if (uriString.startsWith("/")) {
            return Uri.fromFile(new java.io.File(uriString)).toString();
        }
        return uriString;
    }

    private void runOnMainAsync(Runnable action) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            action.run();
            return;
        }
        mainHandler.post(action);
    }

    private void runOnMainBlocking(Runnable action) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            action.run();
            return;
        }
        final Object lock = new Object();
        final Throwable[] error = new Throwable[1];
        final boolean[] done = new boolean[1];
        synchronized (lock) {
            mainHandler.post(() -> {
                try {
                    action.run();
                } catch (Throwable t) {
                    error[0] = t;
                } finally {
                    synchronized (lock) {
                        done[0] = true;
                        lock.notifyAll();
                    }
                }
            });
            try {
                long deadline = System.currentTimeMillis() + BLOCKING_TIMEOUT_MS;
                while (!done[0]) {
                    long remaining = deadline - System.currentTimeMillis();
                    if (remaining <= 0) {
                        throw new RuntimeException("Timed out waiting for ExoPlayer main-thread work");
                    }
                    lock.wait(remaining);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new RuntimeException("Interrupted waiting for ExoPlayer main-thread work", e);
            }
        }
        if (error[0] != null) {
            if (error[0] instanceof RuntimeException) {
                throw (RuntimeException) error[0];
            }
            throw new RuntimeException(error[0]);
        }
    }
}
