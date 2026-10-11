package app.bmdarklight.wave;

import android.content.ContentResolver;
import android.content.Context;
import android.database.Cursor;
import android.media.MediaMetadataRetriever;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.util.Base64;
import android.util.Log;

import androidx.annotation.Keep;
import androidx.annotation.Nullable;

import org.json.JSONObject;

/**
 * Probe title/artist/album/duration/cover from a {@code content://} URI without
 * copying the file into app-private storage (MediaMetadataRetriever).
 */
@Keep
public final class MediaMetadataProbe {
    private static final String TAG = "MediaMetadataProbe";
    private static final int MAX_COVER_BYTES = 8 * 1024 * 1024;

    private MediaMetadataProbe() {}

    /**
     * @return JSON object string with metadata fields (never null)
     */
    @Keep
    public static String probe(@Nullable Context context, @Nullable String uriString) {
        if (context == null) {
            throw new IllegalStateException("MediaMetadataProbe: Context is null");
        }
        if (uriString == null || uriString.trim().isEmpty()) {
            throw new IllegalArgumentException("MediaMetadataProbe: URI is empty");
        }

        Uri uri = Uri.parse(uriString.trim());
        if (uri.getScheme() == null || !"content".equalsIgnoreCase(uri.getScheme())) {
            throw new IllegalArgumentException(
                "MediaMetadataProbe: expected content:// URI, got: " + uriString);
        }

        ContentResolver resolver = context.getContentResolver();
        String displayName = queryDisplayName(resolver, uri);
        long fileSize = queryFileSize(resolver, uri);

        MediaMetadataRetriever retriever = new MediaMetadataRetriever();
        try {
            retriever.setDataSource(context, uri);

            JSONObject out = new JSONObject();
            out.put("title", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_TITLE)));
            out.put("artist", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_ARTIST)));
            out.put("album", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_ALBUM)));
            out.put("albumArtist", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST)));
            out.put("genre", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_GENRE)));
            out.put("year", parseIntOrNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_YEAR)));
            out.put("trackNumber", parseTrackNumber(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_CD_TRACK_NUMBER)));
            out.put("durationMs", parseLongOrNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)));
            out.put("mime", emptyToNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_MIMETYPE)));
            out.put("displayName", displayName);
            out.put("fileSize", fileSize);

            byte[] art = retriever.getEmbeddedPicture();
            if (art != null && art.length > 0 && art.length <= MAX_COVER_BYTES) {
                out.put("coverBase64", Base64.encodeToString(art, Base64.NO_WRAP));
                out.put("coverMime", "image/jpeg");
            }

            return out.toString();
        } catch (Exception e) {
            Log.e(TAG, "probe failed for " + uriString + ": " + e.getMessage());
            throw new RuntimeException("MediaMetadataProbe failed: " + e.getMessage(), e);
        } finally {
            try {
                retriever.release();
            } catch (Exception ignored) {
            }
        }
    }

    @Nullable
    private static String queryDisplayName(ContentResolver resolver, Uri uri) {
        try (Cursor cursor = resolver.query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (idx >= 0) {
                    return emptyToNull(cursor.getString(idx));
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "DISPLAY_NAME query failed: " + e.getMessage());
        }
        return null;
    }

    private static long queryFileSize(ContentResolver resolver, Uri uri) {
        try (Cursor cursor = resolver.query(uri, new String[]{OpenableColumns.SIZE}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int idx = cursor.getColumnIndex(OpenableColumns.SIZE);
                if (idx >= 0 && !cursor.isNull(idx)) {
                    return cursor.getLong(idx);
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "SIZE query failed: " + e.getMessage());
        }
        return 0L;
    }

    @Nullable
    private static String emptyToNull(@Nullable String value) {
        if (value == null) {
            return null;
        }
        String trimmed = value.trim();
        return trimmed.isEmpty() ? null : trimmed;
    }

    @Nullable
    private static Integer parseIntOrNull(@Nullable String value) {
        String v = emptyToNull(value);
        if (v == null) {
            return null;
        }
        // Year sometimes arrives as "2020" or longer date strings.
        String digits = v.length() >= 4 ? v.substring(0, 4) : v;
        try {
            return Integer.parseInt(digits.replaceAll("[^0-9]", ""));
        } catch (Exception e) {
            return null;
        }
    }

    @Nullable
    private static Integer parseTrackNumber(@Nullable String value) {
        String v = emptyToNull(value);
        if (v == null) {
            return null;
        }
        // Often "3/12"
        int slash = v.indexOf('/');
        String head = slash >= 0 ? v.substring(0, slash) : v;
        try {
            return Integer.parseInt(head.trim());
        } catch (Exception e) {
            return null;
        }
    }

    @Nullable
    private static Long parseLongOrNull(@Nullable String value) {
        String v = emptyToNull(value);
        if (v == null) {
            return null;
        }
        try {
            return Long.parseLong(v);
        } catch (Exception e) {
            return null;
        }
    }
}
