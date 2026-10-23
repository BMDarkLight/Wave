package app.bmdarklight.wave;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;

import androidx.annotation.Keep;
import androidx.core.app.NotificationCompat;
import androidx.core.content.FileProvider;

import java.io.File;
import java.util.Locale;

/**
 * The notifications an app update goes through: a quiet one with progress
 * while the APK downloads, then one that alerts when it is ready, which opens
 * the installer when tapped. Both use the same id, so the second replaces the
 * first. Posting needs POST_NOTIFICATIONS, which the media session already
 * asks for; without it these calls do nothing and Settings still shows the
 * update.
 */
@Keep
public final class UpdateNotifier {
    private static final String PROGRESS_CHANNEL = "wave-update-progress";
    private static final String READY_CHANNEL = "wave-update-ready";
    private static final int NOTIFICATION_ID = 0x57617665;
    private static final String APK_TYPE = "application/vnd.android.package-archive";

    private UpdateNotifier() {}

    /** Shows how far the download of `version` has come. */
    public static void progress(Context context, String version, long received, long total) {
        try {
            NotificationManager manager = manager(context);
            if (manager == null) {
                return;
            }
            boolean known = total > 0;
            int percent = known ? (int) Math.min(100, received * 100 / total) : 0;
            NotificationCompat.Builder builder =
                    new NotificationCompat.Builder(context, PROGRESS_CHANNEL)
                            .setSmallIcon(android.R.drawable.stat_sys_download)
                            .setContentTitle("Downloading Wave " + version)
                            .setContentText(known
                                    ? megabytes(received) + " of " + megabytes(total)
                                    : megabytes(received))
                            .setProgress(100, percent, !known)
                            .setOngoing(true)
                            .setOnlyAlertOnce(true)
                            .setSilent(true)
                            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
                            .setPriority(NotificationCompat.PRIORITY_LOW);
            manager.notify(NOTIFICATION_ID, builder.build());
        } catch (Throwable ignored) {
            // A missing notification never stops the download.
        }
    }

    /** Announces that `version` is downloaded; tapping it installs `path`. */
    public static void ready(Context context, String version, String path) {
        try {
            NotificationManager manager = manager(context);
            if (manager == null) {
                return;
            }
            Uri uri = FileProvider.getUriForFile(
                    context, context.getPackageName() + ".wave-updates", new File(path));
            Intent install = new Intent(Intent.ACTION_VIEW)
                    .setDataAndType(uri, APK_TYPE)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                            | Intent.FLAG_ACTIVITY_NEW_TASK);
            PendingIntent tap = PendingIntent.getActivity(
                    context,
                    0,
                    install,
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
            NotificationCompat.Builder builder =
                    new NotificationCompat.Builder(context, READY_CHANNEL)
                            .setSmallIcon(icon(context))
                            .setContentTitle("Wave " + version + " is ready to install")
                            .setContentText("Tap to install the update")
                            .setContentIntent(tap)
                            .setAutoCancel(true)
                            .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
                            .setPriority(NotificationCompat.PRIORITY_HIGH);
            manager.notify(NOTIFICATION_ID, builder.build());
        } catch (Throwable ignored) {
            // Settings still offers the install.
        }
    }

    /** Replaces the progress with why the download of `version` failed. */
    public static void failed(Context context, String version, String reason) {
        try {
            NotificationManager manager = manager(context);
            if (manager == null) {
                return;
            }
            NotificationCompat.Builder builder =
                    new NotificationCompat.Builder(context, PROGRESS_CHANNEL)
                            .setSmallIcon(icon(context))
                            .setContentTitle("Couldn't download Wave " + version)
                            .setContentText(reason)
                            .setStyle(new NotificationCompat.BigTextStyle().bigText(reason))
                            .setAutoCancel(true)
                            .setSilent(true)
                            .setPriority(NotificationCompat.PRIORITY_LOW);
            manager.notify(NOTIFICATION_ID, builder.build());
        } catch (Throwable ignored) {
            // Settings still shows the error.
        }
    }

    /**
     * Whether Wave may post notifications. On Android 13 and later that needs
     * the runtime permission, which is only asked for once music plays.
     */
    public static boolean enabled(Context context) {
        try {
            NotificationManager manager =
                    (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
            return manager != null && manager.areNotificationsEnabled();
        } catch (Throwable t) {
            return false;
        }
    }

    /** Takes down whatever update notification is showing. */
    public static void cancel(Context context) {
        try {
            NotificationManager manager = manager(context);
            if (manager != null) {
                manager.cancel(NOTIFICATION_ID);
            }
        } catch (Throwable ignored) {
            // Nothing to take down.
        }
    }

    /** The notification manager, with both channels in place. */
    private static NotificationManager manager(Context context) {
        NotificationManager manager =
                (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) {
            return null;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel progress = new NotificationChannel(
                    PROGRESS_CHANNEL, "Update downloads", NotificationManager.IMPORTANCE_LOW);
            progress.setDescription("Progress while a new version of Wave downloads");
            progress.setShowBadge(false);
            manager.createNotificationChannel(progress);
            NotificationChannel ready = new NotificationChannel(
                    READY_CHANNEL, "Updates ready", NotificationManager.IMPORTANCE_HIGH);
            ready.setDescription("When a new version of Wave is ready to install");
            manager.createNotificationChannel(ready);
        }
        return manager;
    }

    /** Wave's own notification icon, or the system's when it can't be found. */
    private static int icon(Context context) {
        int id = context.getResources()
                .getIdentifier("ic_notification", "drawable", context.getPackageName());
        return id != 0 ? id : android.R.drawable.stat_sys_download_done;
    }

    private static String megabytes(long bytes) {
        return String.format(Locale.getDefault(), "%.1f MB", bytes / 1_000_000.0);
    }
}
