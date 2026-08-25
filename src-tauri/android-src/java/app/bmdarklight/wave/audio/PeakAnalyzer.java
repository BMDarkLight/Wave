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
 * Offline peak-amplitude scan for volume normalization.
 *
 * Decodes the audio track via {@link MediaExtractor} + {@link MediaCodec} and
 * returns the maximum absolute sample value normalised to 0.0–1.0.
 */
@Keep
public final class PeakAnalyzer {
    private static final String TAG = "PeakAnalyzer";
    private static final long TIMEOUT_US = 10_000L;
    private static final float DEFAULT_PEAK = 0.5f;

    private PeakAnalyzer() {}

    @Keep
    public static float analyzePeak(Context context, String uriString) {
        if (context == null || uriString == null || uriString.trim().isEmpty()) {
            return DEFAULT_PEAK;
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
                return DEFAULT_PEAK;
            }
            extractor.selectTrack(trackIndex);
            MediaFormat format = extractor.getTrackFormat(trackIndex);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime == null) {
                return DEFAULT_PEAK;
            }

            codec = MediaCodec.createDecoderByType(mime);
            codec.configure(format, null, null, 0);
            codec.start();

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            float peak = 0f;
            boolean inputDone = false;

            while (true) {
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
                    peak = Math.max(peak, scanPcmPeak(outBuffer, info.offset, info.size, format));
                }
                codec.releaseOutputBuffer(outIndex, false);
                if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    break;
                }
            }

            return peak > 0f ? Math.min(1f, peak) : DEFAULT_PEAK;
        } catch (Exception e) {
            Log.w(TAG, "Peak analysis failed for " + uriString + ": " + e.getMessage());
            return DEFAULT_PEAK;
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

    private static float scanPcmPeak(ByteBuffer buffer, int offset, int size, MediaFormat format) {
        buffer.position(offset);
        buffer.limit(offset + size);
        buffer.order(ByteOrder.LITTLE_ENDIAN);

        int encoding = 2;
        if (format.containsKey(MediaFormat.KEY_PCM_ENCODING)) {
            encoding = format.getInteger(MediaFormat.KEY_PCM_ENCODING);
        }

        float peak = 0f;
        if (encoding == 4) { // ENCODING_PCM_FLOAT
            while (buffer.remaining() >= 4) {
                peak = Math.max(peak, Math.abs(buffer.getFloat()));
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
                peak = Math.max(peak, Math.abs(sample));
            }
        }
        return peak;
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
