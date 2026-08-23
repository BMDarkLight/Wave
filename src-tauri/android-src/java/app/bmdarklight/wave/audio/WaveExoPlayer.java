package app.bmdarklight.wave.audio;

import android.content.Context;
import android.media.audiofx.Equalizer;
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
 *
 * Crossfade uses a second ExoPlayer instance: the incoming track starts
 * during the fade window while the outgoing track ramps down.
 *
 * EQ uses {@link Equalizer} attached to each player's audio session,
 * mapping Wave's fixed 10-band gains onto the device band layout.
 */
public final class WaveExoPlayer {
    private static final String TAG = "WaveExoPlayer";
    /** Allow slower main-thread work during library sync / WebView load. */
    private static final long BLOCKING_TIMEOUT_MS = 1500L;
    private static final long POSITION_TICK_MS = 250L;
    /** Wave desktop EQ centre frequencies (Hz) — must match `dsp.rs`. */
    private static final float[] WAVE_EQ_HZ = {
            31f, 62f, 125f, 250f, 500f, 1000f, 2000f, 4000f, 8000f, 16000f
    };

    private static volatile WaveExoPlayer INSTANCE;

    private final Context appContext;
    private final Handler mainHandler;
    private ExoPlayer player;
    private ExoPlayer crossfadePlayer;
    private Equalizer playerEq;
    private Equalizer crossfadeEq;
    private int playerEqSessionId = C.AUDIO_SESSION_ID_UNSET;
    private int crossfadeEqSessionId = C.AUDIO_SESSION_ID_UNSET;
    private volatile boolean ended;
    private volatile boolean playingCached;
    private volatile long positionMsCached;
    private volatile long durationMsCached;
    private volatile float crossfadeDurationSec = 0f;
    private volatile String upcomingUri = null;
    private volatile boolean crossfadeActive = false;
    private volatile long crossfadeStartMs = 0L;
    private volatile long crossfadeWindowMs = 0L;
    private volatile String pendingHandoffUri = null;
    private volatile boolean gaplessEnabled = true;
    private volatile int pendingMediaIndexChange = -1;
    private volatile int lastReportedMediaIndex = -1;
    private float userVolume = 1f;
    private volatile boolean eqEnabled = false;
    private final float[] eqBandsDb = new float[WAVE_EQ_HZ.length];

    private final Runnable positionTicker = new Runnable() {
        @Override
        public void run() {
            if (player == null) {
                return;
            }
            if (player.isPlaying() || crossfadeActive) {
                refreshCacheFromPlayer();
                maybeAdvanceCrossfade();
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

    /** Used by the media-session FGS to decide whether to stay alive after close. */
    public static boolean isPlayingActive() {
        WaveExoPlayer inst = INSTANCE;
        return inst != null && inst.isPlaying();
    }

    private AudioAttributes buildAudioAttributes() {
        return new AudioAttributes.Builder()
                .setUsage(C.USAGE_MEDIA)
                .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
                .build();
    }

    private void initPlayer() {
        if (player != null) {
            return;
        }
        player = new ExoPlayer.Builder(appContext)
                .setAudioAttributes(buildAudioAttributes(), /* handleAudioFocus= */ true)
                .setHandleAudioBecomingNoisy(true)
                .build();

        player.addListener(new Player.Listener() {
            @Override
            public void onPlaybackStateChanged(int playbackState) {
                if (!crossfadeActive) {
                    ended = playbackState == Player.STATE_ENDED;
                }
                refreshCacheFromPlayer();
                syncPositionTicker();
            }

            @Override
            public void onIsPlayingChanged(boolean isPlaying) {
                playingCached = isPlaying || (crossfadePlayer != null && crossfadePlayer.isPlaying());
                refreshCacheFromPlayer();
                syncPositionTicker();
            }

            @Override
            public void onPlayerError(PlaybackException error) {
                Log.e(TAG, "Playback error: " + error.getMessage());
                ended = true;
                playingCached = false;
                cancelCrossfadeInternal();
                stopPositionTicker();
            }

            @Override
            public void onMediaItemTransition(MediaItem mediaItem, int reason) {
                attachEqualizerForPlayer(player, /* crossfade= */ false);
                if (!gaplessEnabled || crossfadeActive || player == null) {
                    return;
                }
                int index = player.getCurrentMediaItemIndex();
                if (index != lastReportedMediaIndex) {
                    lastReportedMediaIndex = index;
                    pendingMediaIndexChange = index;
                }
            }

            @Override
            public void onAudioSessionIdChanged(int audioSessionId) {
                attachEqualizerForPlayer(player, /* crossfade= */ false);
            }
        });
        attachEqualizerForPlayer(player, /* crossfade= */ false);
    }

    private ExoPlayer ensureCrossfadePlayer() {
        if (crossfadePlayer == null) {
            crossfadePlayer = new ExoPlayer.Builder(appContext)
                    .setAudioAttributes(buildAudioAttributes(), /* handleAudioFocus= */ false)
                    .setHandleAudioBecomingNoisy(false)
                    .build();
            crossfadePlayer.addListener(new Player.Listener() {
                @Override
                public void onAudioSessionIdChanged(int audioSessionId) {
                    attachEqualizerForPlayer(crossfadePlayer, /* crossfade= */ true);
                }
            });
        }
        attachEqualizerForPlayer(crossfadePlayer, /* crossfade= */ true);
        return crossfadePlayer;
    }

    /** Apply Wave's 10-band gains (dB) to the platform equalizer. */
    public void setEqBands(float[] bandsDb) {
        if (bandsDb == null) {
            return;
        }
        int n = Math.min(eqBandsDb.length, bandsDb.length);
        for (int i = 0; i < n; i++) {
            eqBandsDb[i] = clampEqGainDb(bandsDb[i]);
        }
        runOnMainAsync(() -> {
            applyEqToInstance(playerEq);
            applyEqToInstance(crossfadeEq);
        });
    }

    public void setEqEnabled(boolean enabled) {
        eqEnabled = enabled;
        runOnMainAsync(() -> {
            setEqInstanceEnabled(playerEq, enabled);
            setEqInstanceEnabled(crossfadeEq, enabled);
        });
    }

    private static float clampEqGainDb(float gainDb) {
        if (gainDb > 12f) {
            return 12f;
        }
        if (gainDb < -12f) {
            return -12f;
        }
        return gainDb;
    }

    private void attachEqualizerForPlayer(ExoPlayer target, boolean crossfade) {
        if (target == null) {
            return;
        }
        int sessionId = target.getAudioSessionId();
        if (sessionId == C.AUDIO_SESSION_ID_UNSET || sessionId == 0) {
            return;
        }
        int boundSession = crossfade ? crossfadeEqSessionId : playerEqSessionId;
        Equalizer existing = crossfade ? crossfadeEq : playerEq;
        if (existing != null && boundSession == sessionId) {
            applyEqToInstance(existing);
            setEqInstanceEnabled(existing, eqEnabled);
            return;
        }
        if (existing != null) {
            releaseEqualizer(existing);
            if (crossfade) {
                crossfadeEq = null;
                crossfadeEqSessionId = C.AUDIO_SESSION_ID_UNSET;
            } else {
                playerEq = null;
                playerEqSessionId = C.AUDIO_SESSION_ID_UNSET;
            }
        }
        try {
            Equalizer eq = new Equalizer(0, sessionId);
            applyEqToInstance(eq);
            setEqInstanceEnabled(eq, eqEnabled);
            if (crossfade) {
                crossfadeEq = eq;
                crossfadeEqSessionId = sessionId;
            } else {
                playerEq = eq;
                playerEqSessionId = sessionId;
            }
        } catch (RuntimeException e) {
            Log.w(TAG, "Equalizer unavailable: " + e.getMessage());
        }
    }

    private void applyEqToInstance(Equalizer eq) {
        if (eq == null) {
            return;
        }
        try {
            short bands = eq.getNumberOfBands();
            if (bands <= 0) {
                return;
            }
            short[] range = eq.getBandLevelRange();
            short minLevel = range[0];
            short maxLevel = range[1];
            for (short band = 0; band < bands; band++) {
                float centerHz = eq.getCenterFreq(band) / 1000f;
                float gainDb = interpolateWaveGainDb(centerHz);
                short levelMb = (short) Math.round(gainDb * 100f);
                if (levelMb < minLevel) {
                    levelMb = minLevel;
                } else if (levelMb > maxLevel) {
                    levelMb = maxLevel;
                }
                eq.setBandLevel(band, levelMb);
            }
        } catch (RuntimeException e) {
            Log.w(TAG, "Failed to apply EQ bands: " + e.getMessage());
        }
    }

    private float interpolateWaveGainDb(float centerHz) {
        if (centerHz <= WAVE_EQ_HZ[0]) {
            return eqBandsDb[0];
        }
        int last = WAVE_EQ_HZ.length - 1;
        if (centerHz >= WAVE_EQ_HZ[last]) {
            return eqBandsDb[last];
        }
        for (int i = 0; i < last; i++) {
            float lo = WAVE_EQ_HZ[i];
            float hi = WAVE_EQ_HZ[i + 1];
            if (centerHz >= lo && centerHz <= hi) {
                float t = (centerHz - lo) / (hi - lo);
                return eqBandsDb[i] + (eqBandsDb[i + 1] - eqBandsDb[i]) * t;
            }
        }
        return 0f;
    }

    private static void setEqInstanceEnabled(Equalizer eq, boolean enabled) {
        if (eq == null) {
            return;
        }
        try {
            eq.setEnabled(enabled);
        } catch (RuntimeException e) {
            Log.w(TAG, "Failed to toggle EQ: " + e.getMessage());
        }
    }

    private static void releaseEqualizer(Equalizer eq) {
        if (eq == null) {
            return;
        }
        try {
            eq.setEnabled(false);
            eq.release();
        } catch (RuntimeException e) {
            Log.w(TAG, "Failed to release EQ: " + e.getMessage());
        }
    }

    public void setGaplessEnabled(boolean enabled) {
        gaplessEnabled = enabled;
    }

    /** Returns and clears a pending playlist index change (gapless queue advance). */
    public int consumeMediaIndexChange() {
        int index = pendingMediaIndexChange;
        pendingMediaIndexChange = -1;
        return index;
    }

    public boolean hasNextMediaItem() {
        if (player == null) {
            return false;
        }
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return player.hasNextMediaItem();
        }
        final boolean[] result = new boolean[1];
        runOnMainBlocking(() -> result[0] = player != null && player.hasNextMediaItem());
        return result[0];
    }

    public boolean seekToNextMediaItem() {
        final boolean[] result = new boolean[1];
        runOnMainBlocking(() -> {
            if (player != null && player.hasNextMediaItem()) {
                cancelCrossfadeInternal();
                ended = false;
                player.seekToNextMediaItem();
                lastReportedMediaIndex = player.getCurrentMediaItemIndex();
                pendingMediaIndexChange = lastReportedMediaIndex;
                refreshCacheFromPlayer();
                syncPositionTicker();
                result[0] = true;
            }
        });
        return result[0];
    }

    public int getCurrentMediaIndex() {
        if (player == null) {
            return 0;
        }
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return player.getCurrentMediaItemIndex();
        }
        final int[] result = new int[1];
        runOnMainBlocking(() -> result[0] = player != null ? player.getCurrentMediaItemIndex() : 0);
        return result[0];
    }

    public void playMediaItems(String[] uriStrings, int startIndex) {
        if (uriStrings == null || uriStrings.length == 0) {
            throw new IllegalArgumentException("uris is empty");
        }
        runOnMainBlocking(() -> {
            initPlayer();
            cancelCrossfadeInternal();
            ended = false;
            java.util.ArrayList<MediaItem> items = new java.util.ArrayList<>();
            for (String uriString : uriStrings) {
                if (uriString != null && !uriString.isEmpty()) {
                    items.add(MediaItem.fromUri(Uri.parse(normalizeUri(uriString))));
                }
            }
            if (items.isEmpty()) {
                throw new IllegalArgumentException("uris is empty");
            }
            int idx = Math.max(0, Math.min(startIndex, items.size() - 1));
            player.stop();
            player.clearMediaItems();
            player.setMediaItems(items, idx, 0L);
            player.setVolume(userVolume);
            player.prepare();
            player.play();
            lastReportedMediaIndex = idx;
            pendingMediaIndexChange = -1;
            playingCached = true;
            attachEqualizerForPlayer(player, /* crossfade= */ false);
            refreshCacheFromPlayer();
            syncPositionTicker();
        });
    }

    public void setCrossfadeDuration(float seconds) {
        crossfadeDurationSec = Math.max(0f, Math.min(8f, seconds));
    }

    public void setUpcomingUri(String uriString) {
        if (uriString == null || uriString.isEmpty()) {
            upcomingUri = null;
        } else {
            upcomingUri = uriString;
        }
    }

    /** Returns and clears a pending logical track handoff (fade start). */
    public String consumeCrossfadeHandoff() {
        String uri = pendingHandoffUri;
        pendingHandoffUri = null;
        return uri;
    }

    private void refreshCacheFromPlayer() {
        if (player == null) {
            return;
        }
        if (crossfadeActive && crossfadePlayer != null && crossfadePlayer.isPlaying()) {
            playingCached = true;
            positionMsCached = crossfadePlayer.getCurrentPosition();
            long d = crossfadePlayer.getDuration();
            durationMsCached = d == C.TIME_UNSET ? 0L : d;
            return;
        }
        playingCached = player.isPlaying();
        positionMsCached = player.getCurrentPosition();
        long d = player.getDuration();
        durationMsCached = d == C.TIME_UNSET ? 0L : d;
    }

    private void syncPositionTicker() {
        stopPositionTicker();
        if (player != null && (player.isPlaying() || crossfadeActive)) {
            mainHandler.postDelayed(positionTicker, POSITION_TICK_MS);
        }
    }

    private void stopPositionTicker() {
        mainHandler.removeCallbacks(positionTicker);
    }

    private void cancelCrossfadeInternal() {
        crossfadeActive = false;
        crossfadeStartMs = 0L;
        crossfadeWindowMs = 0L;
        pendingHandoffUri = null;
        if (crossfadePlayer != null) {
            crossfadePlayer.stop();
            crossfadePlayer.clearMediaItems();
            crossfadePlayer.setVolume(0f);
        }
        if (player != null) {
            player.setVolume(userVolume);
        }
    }

    private void maybeAdvanceCrossfade() {
        if (crossfadeActive) {
            updateCrossfadeVolumes();
            return;
        }
        if (crossfadeDurationSec <= 0f || upcomingUri == null || player == null || !player.isPlaying()) {
            return;
        }
        long duration = durationMsCached;
        long position = positionMsCached;
        if (duration <= 0L) {
            return;
        }
        long windowMs = (long) (crossfadeDurationSec * 1000f);
        if (windowMs <= 0L) {
            return;
        }
        if (position + 50L < duration - windowMs) {
            return;
        }
        startCrossfade();
    }

    private void startCrossfade() {
        if (upcomingUri == null || crossfadeActive) {
            return;
        }
        String nextUri = upcomingUri;
        upcomingUri = null;
        crossfadeActive = true;
        crossfadeStartMs = System.currentTimeMillis();
        long remaining = Math.max(250L, durationMsCached - positionMsCached);
        crossfadeWindowMs = Math.min((long) (crossfadeDurationSec * 1000f), remaining);
        pendingHandoffUri = nextUri;

        ExoPlayer incoming = ensureCrossfadePlayer();
        incoming.stop();
        incoming.clearMediaItems();
        incoming.setMediaItem(MediaItem.fromUri(Uri.parse(normalizeUri(nextUri))));
        incoming.prepare();
        incoming.setVolume(0f);
        incoming.play();
        attachEqualizerForPlayer(incoming, /* crossfade= */ true);
        updateCrossfadeVolumes();
        ended = false;
    }

    private void updateCrossfadeVolumes() {
        if (!crossfadeActive || crossfadePlayer == null || player == null) {
            return;
        }
        long elapsed = System.currentTimeMillis() - crossfadeStartMs;
        float progress = crossfadeWindowMs > 0L
                ? Math.min(1f, (float) elapsed / (float) crossfadeWindowMs)
                : 1f;
        float outVol = userVolume * (1f - progress);
        float inVol = userVolume * progress;
        player.setVolume(outVol);
        crossfadePlayer.setVolume(inVol);
        if (progress >= 1f) {
            completeCrossfade();
        }
    }

    private void completeCrossfade() {
        if (!crossfadeActive || crossfadePlayer == null || player == null) {
            cancelCrossfadeInternal();
            return;
        }
        ExoPlayer outgoing = player;
        Equalizer outgoingEq = playerEq;
        int outgoingEqSession = playerEqSessionId;
        player = crossfadePlayer;
        playerEq = crossfadeEq;
        playerEqSessionId = crossfadeEqSessionId;
        crossfadePlayer = outgoing;
        crossfadeEq = outgoingEq;
        crossfadeEqSessionId = outgoingEqSession;

        player.setVolume(userVolume);
        crossfadePlayer.stop();
        crossfadePlayer.clearMediaItems();
        crossfadePlayer.setVolume(0f);
        releaseEqualizer(crossfadeEq);
        crossfadeEq = null;
        crossfadeEqSessionId = C.AUDIO_SESSION_ID_UNSET;
        attachEqualizerForPlayer(player, /* crossfade= */ false);

        crossfadeActive = false;
        crossfadeStartMs = 0L;
        crossfadeWindowMs = 0L;
        ended = false;
        playingCached = player.isPlaying();
        refreshCacheFromPlayer();
        syncPositionTicker();
    }

    public void playUri(String uriString) {
        if (uriString == null || uriString.isEmpty()) {
            throw new IllegalArgumentException("uri is empty");
        }
        runOnMainBlocking(() -> {
            initPlayer();
            cancelCrossfadeInternal();
            ended = false;
            Uri uri = Uri.parse(normalizeUri(uriString));
            player.stop();
            player.clearMediaItems();
            player.setMediaItem(MediaItem.fromUri(uri));
            player.setVolume(userVolume);
            player.prepare();
            player.play();
            lastReportedMediaIndex = 0;
            pendingMediaIndexChange = -1;
            playingCached = true;
            attachEqualizerForPlayer(player, /* crossfade= */ false);
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
            cancelCrossfadeInternal();
            ended = false;
            Uri uri = Uri.parse(normalizeUri(uriString));
            player.stop();
            player.clearMediaItems();
            player.setMediaItem(MediaItem.fromUri(uri));
            player.setVolume(userVolume);
            player.prepare();
            long clamped = Math.max(0L, positionMs);
            player.seekTo(clamped);
            player.pause();
            playingCached = false;
            positionMsCached = clamped;
            attachEqualizerForPlayer(player, /* crossfade= */ false);
            refreshCacheFromPlayer();
            stopPositionTicker();
        });
    }

    public void play() {
        runOnMainBlocking(() -> {
            if (player != null) {
                ended = false;
                player.play();
                if (crossfadePlayer != null && crossfadeActive) {
                    crossfadePlayer.play();
                }
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
                if (crossfadePlayer != null) {
                    crossfadePlayer.pause();
                }
                playingCached = false;
                stopPositionTicker();
            }
        });
    }

    public void stop() {
        runOnMainBlocking(() -> {
            cancelCrossfadeInternal();
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
            cancelCrossfadeInternal();
            if (player != null) {
                ended = false;
                long clamped = Math.max(0L, positionMs);
                player.seekTo(clamped);
                positionMsCached = clamped;
            }
        });
    }

    public void setVolume(float volume) {
        userVolume = Math.max(0f, Math.min(1f, volume));
        runOnMainAsync(() -> {
            if (player != null && !crossfadeActive) {
                player.setVolume(userVolume);
            }
        });
    }

    public long getCurrentPosition() {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            refreshCacheFromPlayer();
        }
        return positionMsCached;
    }

    public long getDuration() {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            refreshCacheFromPlayer();
        }
        return durationMsCached;
    }

    public boolean isPlaying() {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            refreshCacheFromPlayer();
        }
        return playingCached;
    }

    public boolean isEnded() {
        return ended && !crossfadeActive;
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
