package app.bmdarklight.wave;

import android.content.Context;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import androidx.annotation.Keep;

/**
 * Opens a SAF document for reading through the Application context, so it
 * works with no window open: sharing the library in the background serves
 * `content://` songs this way. The file descriptor is handed to Rust, which
 * owns and closes it.
 */
@Keep
public final class SafFile {
    private SafFile() {}

    /** A detached read-only file descriptor for `uri`, or -1. */
    public static int openForReading(Context context, String uri) {
        try {
            ParcelFileDescriptor fd =
                    context.getContentResolver().openFileDescriptor(Uri.parse(uri), "r");
            return fd != null ? fd.detachFd() : -1;
        } catch (Throwable t) {
            return -1;
        }
    }
}
