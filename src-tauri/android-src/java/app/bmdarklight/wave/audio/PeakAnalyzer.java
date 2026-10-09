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
 * Offline peak + RMS amplitude scan for volume normalization, and per-block
 * peaks for the seek bar waveform.
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
    // A waveform needs the whole track, so it gets a longer cap. Past it the
    // shape covers only the part decoded so far.
    private static final long MAX_WAVEFORM_SCAN_MS = 60_000L;
    private static final float WAVEFORM_BLOCK_SECS = 0.02f;

    private PeakAnalyzer() {}

    /** Running peak/RMS accumulator for one scan. */
    private static final class Accumulator {
        float peak = 0f;
        double sumSquares = 0.0;
        long count = 0L;
    }

    /** Receives each decoded sample as -1.0 to 1.0, with the output format. */
    private interface SampleConsumer {
        void format(MediaFormat format);

        void sample(float value);
    }

    /** Returns {peak, rms}, both 0.0-1.0. */
    @Keep
    public static float[] analyzeLevels(Context context, String uriString) {
        Accumulator acc = new Accumulator();
        boolean decoded = decode(context, uriString, MAX_SCAN_MS, new SampleConsumer() {
            @Override
            public void format(MediaFormat format) {}

            @Override
            public void sample(float value) {
                float abs = Math.abs(value);
                acc.peak = Math.max(acc.peak, abs);
                acc.sumSquares += (double) abs * abs;
                acc.count++;
            }
        });
        if (!decoded) {
            return new float[] {DEFAULT_PEAK, DEFAULT_RMS};
        }
        float peak = acc.peak > 0f ? Math.min(1f, acc.peak) : DEFAULT_PEAK;
        float rms = acc.count > 0
                ? Math.min(1f, (float) Math.sqrt(acc.sumSquares / acc.count))
                : DEFAULT_RMS;
        return new float[] {peak, rms};
    }

    /**
     * Peak level of every {@link #WAVEFORM_BLOCK_SECS} of the track, 0.0-1.0,
     * for drawing its waveform. Empty when the file cannot be decoded.
     */
    @Keep
    public static float[] analyzeWaveform(Context context, String uriString) {
        WaveformBlocks blocks = new WaveformBlocks();
        if (!decode(context, uriString, MAX_WAVEFORM_SCAN_MS, blocks)) {
            return new float[0];
        }
        return blocks.toArray();
    }

    /** Groups samples into blocks and keeps the loudest of each. */
    private static final class WaveformBlocks implements SampleConsumer {
        private float[] peaks = new float[4096];
        private int size = 0;
        private int blockSamples = 1;
        private int count = 0;
        private float loudest = 0f;

        @Override
        public void format(MediaFormat format) {
            int rate = format.containsKey(MediaFormat.KEY_SAMPLE_RATE)
                    ? format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                    : 44100;
            int channels = format.containsKey(MediaFormat.KEY_CHANNEL_COUNT)
                    ? format.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                    : 2;
            blockSamples = Math.max(1, (int) (rate * WAVEFORM_BLOCK_SECS) * Math.max(1, channels));
        }

        @Override
        public void sample(float value) {
            loudest = Math.max(loudest, Math.abs(value));
            if (++count >= blockSamples) {
                push();
            }
        }

        private void push() {
            if (size == peaks.length) {
                float[] grown = new float[peaks.length * 2];
                System.arraycopy(peaks, 0, grown, 0, size);
                peaks = grown;
            }
            peaks[size++] = Math.min(1f, loudest);
            loudest = 0f;
            count = 0;
        }

        float[] toArray() {
            if (count > 0) {
                push();
            }
            float[] out = new float[size];
            System.arraycopy(peaks, 0, out, 0, size);
            return out;
        }
    }

    /**
     * Decode the first audio track of {@code uriString} and hand every sample
     * to {@code consumer}, stopping after {@code maxMs} of wall time. Returns
     * false when the file could not be opened or decoded at all.
     */
    private static boolean decode(
            Context context, String uriString, long maxMs, SampleConsumer consumer) {
        if (context == null || uriString == null || uriString.trim().isEmpty()) {
            return false;
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
                return false;
            }
            extractor.selectTrack(trackIndex);
            MediaFormat format = extractor.getTrackFormat(trackIndex);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime == null) {
                return false;
            }

            codec = MediaCodec.createDecoderByType(mime);
            codec.configure(format, null, null, 0);
            codec.start();
            consumer.format(format);

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            boolean inputDone = false;
            long deadline = System.currentTimeMillis() + maxMs;

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
                    // The decoded format (sample rate, channels, encoding) can
                    // differ from the container's.
                    format = codec.getOutputFormat();
                    consumer.format(format);
                    continue;
                }
                if (outIndex < 0) {
                    continue;
                }

                ByteBuffer outBuffer = codec.getOutputBuffer(outIndex);
                if (outBuffer != null && info.size > 0) {
                    readSamples(outBuffer, info.offset, info.size, format, consumer);
                }
                codec.releaseOutputBuffer(outIndex, false);
                if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    break;
                }
            }
            return true;
        } catch (Exception e) {
            Log.w(TAG, "Decoding failed for " + uriString + ": " + e.getMessage());
            return false;
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

    private static void readSamples(
            ByteBuffer buffer, int offset, int size, MediaFormat format, SampleConsumer consumer) {
        buffer.position(offset);
        buffer.limit(offset + size);
        buffer.order(ByteOrder.LITTLE_ENDIAN);

        int encoding = 2;
        if (format.containsKey(MediaFormat.KEY_PCM_ENCODING)) {
            encoding = format.getInteger(MediaFormat.KEY_PCM_ENCODING);
        }

        if (encoding == 4) { // ENCODING_PCM_FLOAT
            while (buffer.remaining() >= 4) {
                consumer.sample(buffer.getFloat());
            }
        } else {
            int sampleBytes = encoding == 3 ? 4 : 2; // 24-bit treated as 32, else 16-bit
            while (buffer.remaining() >= sampleBytes) {
                if (sampleBytes >= 4) {
                    consumer.sample(buffer.getInt() / 2147483648f);
                } else {
                    consumer.sample(buffer.getShort() / 32768f);
                }
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
