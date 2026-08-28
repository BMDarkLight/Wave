package app.bmdarklight.wave.audio;

import android.content.Context;
import android.media.MediaCodec;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import android.net.Uri;
import android.util.Log;

import androidx.annotation.Keep;

import java.io.File;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/**
 * Offline peak + RMS amplitude scan for volume normalization.
 *
 * Decodes the audio track via {@link MediaExtractor} + {@link MediaCodec} and
 * returns {peak, rms}, both normalised to 0.0–1.0. Peak alone doesn't track
 * perceived loudness (a sparse mix with one loud transient can have a high
 * peak while sounding quiet throughout), so gain is driven by RMS; peak is
 * kept only as a clip-safety guard on boosts.
 */
@Keep
public final class PeakAnalyzer {
    private static final String TAG = "PeakAnalyzer";
    private static final long TIMEOUT_US = 10_000L;
    private static final float DEFAULT_PEAK = 0.5f;
    private static final float DEFAULT_RMS = 0.5f;
    // Hard wall-clock cap on a single scan. Peak amplitude is normally
    // established well within this window; the cap exists so a very long or
    // pathological file can't tie up the background analysis thread
    // indefinitely. Runs off the player lock, so this only bounds one
    // background thread's lifetime, not playback.
    private static final long MAX_SCAN_MS = 8_000L;

    private PeakAnalyzer() {}

    /** Running peak/RMS accumulator for one scan. */
    private static final class Accumulator {
        float peak = 0f;
        double sumSquares = 0.0;
        long count = 0L;
    }

    /** Returns {peak, rms}, both 0.0-1.0. */
    @Keep
    public static float[] analyzeLevels(Context context, String uriString) {
        if (context == null || uriString == null || uriString.trim().isEmpty()) {
            return new float[] {DEFAULT_PEAK, DEFAULT_RMS};
        }
        MediaExtractor extractor = new MediaExtractor();
        MediaCodec codec = null;
        try {
            Uri uri = Uri.parse(normalizeUri(uriString.trim()));
            if ("content".equalsIgnoreCase(uri.getScheme())) {
                extractor.setDataSource(context, uri, null);
            } else {
                extractor.setDataSource(uriString.trim());
            }

            int trackIndex = selectAudioTrack(extractor);
            if (trackIndex < 0) {
                return new float[] {DEFAULT_PEAK, DEFAULT_RMS};
            }
            extractor.selectTrack(trackIndex);
            MediaFormat format = extractor.getTrackFormat(trackIndex);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime == null) {
                return new float[] {DEFAULT_PEAK, DEFAULT_RMS};
            }

            codec = MediaCodec.createDecoderByType(mime);
            codec.configure(format, null, null, 0);
            codec.start();

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            Accumulator acc = new Accumulator();
            boolean inputDone = false;
            long deadline = System.currentTimeMillis() + MAX_SCAN_MS;

            while (true) {
                if (System.currentTimeMillis() > deadline) {
                    break;
                }
                if (!inputDone) {
                    int inIndex = codec.dequeueInputBuffer(TIMEOUT_US);
                    if (inIndex >= 0) {
                        ByteBuffer inBuffer = codec.getInputBuffer(inIndex);
                        if (inBuffer == null) {
                            codec.queueInputBuffer(inIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                            inputDone = true;
                            continue;
                        }
                        int sampleSize = extractor.readSampleData(inBuffer, 0);
                        if (sampleSize < 0) {
                            codec.queueInputBuffer(inIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                            inputDone = true;
                        } else {
                            long pts = extractor.getSampleTime();
                            codec.queueInputBuffer(inIndex, 0, sampleSize, pts, 0);
                            extractor.advance();
                        }
                    }
                }

                int outIndex = codec.dequeueOutputBuffer(info, TIMEOUT_US);
                if (outIndex == MediaCodec.INFO_TRY_AGAIN_LATER) {
                    if (inputDone) {
                        break;
                    }
                    continue;
                }
                if (outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    continue;
                }
                if (outIndex < 0) {
                    continue;
                }

                ByteBuffer outBuffer = codec.getOutputBuffer(outIndex);
                if (outBuffer != null && info.size > 0) {
                    scanPcmLevels(outBuffer, info.offset, info.size, format, acc);
                }
                codec.releaseOutputBuffer(outIndex, false);
                if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    break;
                }
            }

            float peak = acc.peak > 0f ? Math.min(1f, acc.peak) : DEFAULT_PEAK;
            float rms = acc.count > 0
                    ? Math.min(1f, (float) Math.sqrt(acc.sumSquares / acc.count))
                    : DEFAULT_RMS;
            return new float[] {peak, rms};
        } catch (Exception e) {
            Log.w(TAG, "Level analysis failed for " + uriString + ": " + e.getMessage());
            return new float[] {DEFAULT_PEAK, DEFAULT_RMS};
        } finally {
            if (codec != null) {
                try {
                    codec.stop();
                    codec.release();
                } catch (Exception ignored) {
                }
            }
            try {
                extractor.release();
            } catch (Exception ignored) {
            }
        }
    }

    private static int selectAudioTrack(MediaExtractor extractor) {
        for (int i = 0; i < extractor.getTrackCount(); i++) {
            MediaFormat format = extractor.getTrackFormat(i);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime != null && mime.startsWith("audio/")) {
                return i;
            }
        }
        return -1;
    }

    private static void scanPcmLevels(
            ByteBuffer buffer, int offset, int size, MediaFormat format, Accumulator acc) {
        buffer.position(offset);
        buffer.limit(offset + size);
        buffer.order(ByteOrder.LITTLE_ENDIAN);

        int encoding = 2;
        if (format.containsKey(MediaFormat.KEY_PCM_ENCODING)) {
            encoding = format.getInteger(MediaFormat.KEY_PCM_ENCODING);
        }

        if (encoding == 4) { // ENCODING_PCM_FLOAT
            while (buffer.remaining() >= 4) {
                float abs = Math.abs(buffer.getFloat());
                acc.peak = Math.max(acc.peak, abs);
                acc.sumSquares += (double) abs * abs;
                acc.count++;
            }
        } else {
            int sampleBytes = encoding == 3 ? 4 : 2; // 24-bit treated as 32, else 16-bit
            while (buffer.remaining() >= sampleBytes) {
                float sample;
                if (sampleBytes >= 4) {
                    sample = buffer.getInt() / 2147483648f;
                } else {
                    sample = buffer.getShort() / 32768f;
                }
                float abs = Math.abs(sample);
                acc.peak = Math.max(acc.peak, abs);
                acc.sumSquares += (double) abs * abs;
                acc.count++;
            }
        }
    }

    private static String normalizeUri(String uriString) {
        if (uriString.startsWith("content://")
                || uriString.startsWith("file://")
                || uriString.startsWith("http://")
                || uriString.startsWith("https://")) {
            return uriString;
        }
        if (uriString.startsWith("/")) {
            return Uri.fromFile(new File(uriString)).toString();
        }
        return uriString;
    }
}
