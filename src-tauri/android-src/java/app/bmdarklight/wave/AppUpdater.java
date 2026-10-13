package app.bmdarklight.wave;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;

import androidx.annotation.Keep;
import androidx.core.content.FileProvider;

import java.io.File;

/**
 * Hands a downloaded update to Android's package installer, and opens
 * release pages in the browser.
 *
 * Android always asks before it installs. The first time, it also sends the
 * user to Settings to let Wave install apps, then back to the installer.
 */
@Keep
public final class AppUpdater {
    private static final String APK_TYPE = "application/vnd.android.package-archive";

    private AppUpdater() {}

    public static void install(Activity activity, String path) {
        Uri uri = FileProvider.getUriForFile(
                activity, activity.getPackageName() + ".wave-updates", new File(path));
        Intent intent = new Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, APK_TYPE)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        activity.startActivity(intent);
    }

    public static void openUrl(Activity activity, String url) {
        activity.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
    }
}
