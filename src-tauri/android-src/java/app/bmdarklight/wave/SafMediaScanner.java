package app.bmdarklight.wave;

import android.content.ContentResolver;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.util.Log;

import androidx.annotation.Keep;
import androidx.annotation.Nullable;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/**
 * Lists audio files under a SAF tree URI ({@code content://…/tree/…}).
 *
 * Used because {@code tauri-plugin-fs} {@code readDir} cannot list content://
 * trees (it tries to convert them to filesystem paths and fails with
 * "URL is not a valid path").
 */
@Keep
public final class SafMediaScanner {
    private static final String TAG = "SafMediaScanner";
    private static final int MAX_FILES = 50_000;

    private static final Set<String> AUDIO_EXTENSIONS = new HashSet<>();
    static {
        String[] exts = {
            "mp3", "flac", "ogg", "opus", "wav", "m4a", "m4b", "aac",
            "aiff", "aif", "alac", "caf", "mka", "wma", "weba",
        };
        for (String ext : exts) {
            AUDIO_EXTENSIONS.add(ext);
        }
    }

    private SafMediaScanner() {}

    /**
     * JNI entry: recursively list audio document URIs under {@code treeUriString}.
     *
     * @return array of {@code content://…/document/…} URIs (never null)
     */
    @Keep
    public static String[] listAudioFiles(
        @Nullable Context context,
        @Nullable String treeUriString
    ) {
        ArrayList<String> out = new ArrayList<>();
        if (context == null) {
            throw new IllegalStateException("SAF scan: Context is null");
        }
        if (treeUriString == null || treeUriString.trim().isEmpty()) {
            throw new IllegalArgumentException("SAF scan: tree URI is empty");
        }

        Uri treeUri = Uri.parse(treeUriString.trim());
        if (treeUri.getScheme() == null
            || !"content".equalsIgnoreCase(treeUri.getScheme())) {
            throw new IllegalArgumentException(
                "SAF scan: expected content:// tree URI, got: " + treeUriString);
        }

        String treeDocId;
        try {
            treeDocId = DocumentsContract.getTreeDocumentId(treeUri);
        } catch (Exception e) {
            throw new IllegalArgumentException(
                "SAF scan: not a tree URI: " + treeUriString, e);
        }

        ContentResolver resolver = context.getContentResolver();
        walk(resolver, treeUri, treeDocId, out);
        Log.d(TAG, "Listed " + out.size() + " audio file(s) under " + treeUriString);
        return out.toArray(new String[0]);
    }

    private static void walk(
        ContentResolver resolver,
        Uri treeUri,
        String parentDocId,
        ArrayList<String> out
    ) {
        if (out.size() >= MAX_FILES) {
            return;
        }

        Uri childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(
            treeUri, parentDocId);

        final String[] projection = new String[] {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        };

        Cursor cursor = null;
        try {
            cursor = resolver.query(childrenUri, projection, null, null, null);
            if (cursor == null) {
                Log.w(TAG, "query returned null for " + childrenUri);
                return;
            }

            int idIdx = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_DOCUMENT_ID);
            int nameIdx = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_DISPLAY_NAME);
            int mimeIdx = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_MIME_TYPE);
            if (idIdx < 0) {
                return;
            }

            while (cursor.moveToNext()) {
                if (out.size() >= MAX_FILES) {
                    Log.w(TAG, "Hit max file limit (" + MAX_FILES + ")");
                    break;
                }

                String docId = cursor.getString(idIdx);
                if (docId == null || docId.isEmpty()) {
                    continue;
                }

                String mime = mimeIdx >= 0 ? cursor.getString(mimeIdx) : null;
                String name = nameIdx >= 0 ? cursor.getString(nameIdx) : null;

                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mime)) {
                    walk(resolver, treeUri, docId, out);
                    continue;
                }

                if (isAudio(name, mime)) {
                    Uri docUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, docId);
                    out.add(docUri.toString());
                }
            }
        } catch (SecurityException e) {
            Log.e(TAG, "Permission denied listing " + childrenUri + ": " + e.getMessage());
            throw e;
        } catch (Exception e) {
            Log.e(TAG, "Failed listing " + childrenUri + ": " + e.getMessage());
            throw new RuntimeException("SAF scan failed: " + e.getMessage(), e);
        } finally {
            if (cursor != null) {
                cursor.close();
            }
        }
    }

    private static boolean isAudio(@Nullable String name, @Nullable String mime) {
        if (mime != null) {
            String lower = mime.toLowerCase(Locale.US);
            if (lower.startsWith("audio/")) {
                return true;
            }
            // Some providers use application/ogg etc.
            if (lower.equals("application/ogg")
                || lower.equals("application/x-ogg")
                || lower.equals("application/flac")) {
                return true;
            }
        }
        if (name == null) {
            return false;
        }
        int dot = name.lastIndexOf('.');
        if (dot < 0 || dot == name.length() - 1) {
            return false;
        }
        String ext = name.substring(dot + 1).toLowerCase(Locale.US);
        return AUDIO_EXTENSIONS.contains(ext);
    }
}
