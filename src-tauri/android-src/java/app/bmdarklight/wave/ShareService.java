package app.bmdarklight.wave;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;

import androidx.annotation.Keep;
import androidx.core.app.NotificationCompat;
import androidx.core.content.ContextCompat;

/**
 * Keeps Wave running for paired Waves while its window is closed. The
 * server they talk to lives in Rust threads of this process; the service
 * only holds the process in the foreground, with a notification saying what
 * it is doing, so Android doesn't stop it. Type connectedDevice, which the
 * CHANGE_WIFI_MULTICAST_STATE permission sharing already holds qualifies for.
 */
@Keep
public final class ShareService extends Service {
    private static final String CHANNEL = "wave-sharing";
    private static final int NOTIFICATION_ID = 0x53686172;
    private static final String EXTRA_NAME = "name";

    /** Starts the service, or updates its notification when it runs. */
    public static void start(Context context, String name) {
        try {
            Intent intent = new Intent(context, ShareService.class).putExtra(EXTRA_NAME, name);
            ContextCompat.startForegroundService(context, intent);
        } catch (Throwable ignored) {
            // Sharing still runs while Wave is open.
        }
    }

    public static void stop(Context context) {
        try {
            context.stopService(new Intent(context, ShareService.class));
        } catch (Throwable ignored) {
            // Not running.
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String name = intent != null ? intent.getStringExtra(EXTRA_NAME) : null;
        Notification notification = build(name != null ? name : "Wave");
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notification,
                        ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
            } else {
                startForeground(NOTIFICATION_ID, notification);
            }
        } catch (Throwable t) {
            // Android refused the foreground start; sharing lasts as long as
            // the window does.
            stopSelf();
        }
        // If Android ends the process the server ends with it, so there is
        // nothing to restart the service for.
        return START_NOT_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private Notification build(String name) {
        NotificationManager manager =
                (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL, "Wave devices", NotificationManager.IMPORTANCE_LOW);
            channel.setDescription("Shown while your other Waves can reach this one in the background");
            channel.setShowBadge(false);
            manager.createNotificationChannel(channel);
        }
        NotificationCompat.Builder builder = new NotificationCompat.Builder(this, CHANNEL)
                .setSmallIcon(icon())
                .setContentTitle("Reachable by your Wave devices")
                .setContentText("They can play your music and play to “" + name + "”")
                .setOngoing(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_SERVICE)
                .setPriority(NotificationCompat.PRIORITY_LOW);
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        if (open != null) {
            builder.setContentIntent(PendingIntent.getActivity(
                    this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE));
        }
        return builder.build();
    }

    private int icon() {
        int id = getResources().getIdentifier("ic_notification", "drawable", getPackageName());
        return id != 0 ? id : android.R.drawable.stat_sys_upload;
    }
}
