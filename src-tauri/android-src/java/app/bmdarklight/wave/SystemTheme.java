package app.bmdarklight.wave;

import android.app.Activity;
import android.app.Application;
import android.content.ComponentCallbacks;
import android.content.Context;
import android.content.res.Configuration;
import android.os.Bundle;
import android.view.Window;

import androidx.annotation.Keep;
import androidx.annotation.NonNull;
import androidx.annotation.Nullable;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * The phone's light or dark setting, for the "System" theme in Settings, and
 * the status and navigation bar icons to match the theme the page shows.
 *
 * The Activity handles uiMode changes itself, so nothing reloads when the
 * phone switches. The switch reaches the engine as a "theme-changed" action
 * instead, and the engine tells the page.
 */
@Keep
public final class SystemTheme {
    private static final AtomicBoolean INSTALLED = new AtomicBoolean(false);
    /** Null until the page reports its theme; the bars keep Android's default. */
    private static volatile Boolean lightBars;
    private static volatile int nightMode = -1;

    private SystemTheme() {}

    public static void install(Context context) {
        if (context == null || !INSTALLED.compareAndSet(false, true)) {
            return;
        }
        Context app = context.getApplicationContext();
        if (!(app instanceof Application)) {
            return;
        }
        nightMode = nightBits(app.getResources().getConfiguration());
        ((Application) app).registerActivityLifecycleCallbacks(new Callbacks());
        app.registerComponentCallbacks(new ConfigWatcher());
    }

    public static boolean isDark(Context context) {
        return nightBits(context.getResources().getConfiguration())
                == Configuration.UI_MODE_NIGHT_YES;
    }

    /** Dark icons for a light page, light icons for a dark one. */
    public static void setLightBars(Activity activity, boolean light) {
        lightBars = light;
        activity.runOnUiThread(() -> apply(activity));
    }

    private static void apply(Activity activity) {
        Boolean light = lightBars;
        if (light == null || activity.isFinishing() || activity.isDestroyed()) {
            return;
        }
        Window window = activity.getWindow();
        if (window == null) {
            return;
        }
        WindowInsetsControllerCompat bars =
                WindowCompat.getInsetsController(window, window.getDecorView());
        bars.setAppearanceLightStatusBars(light);
        bars.setAppearanceLightNavigationBars(light);
    }

    private static int nightBits(Configuration config) {
        return config.uiMode & Configuration.UI_MODE_NIGHT_MASK;
    }

    private static final class ConfigWatcher implements ComponentCallbacks {
        @Override
        public void onConfigurationChanged(@NonNull Configuration config) {
            int night = nightBits(config);
            if (night != nightMode) {
                nightMode = night;
                MediaNativeBridge.dispatch("theme-changed");
            }
        }

        @Override
        public void onLowMemory() {}
    }

    private static final class Callbacks implements Application.ActivityLifecycleCallbacks {
        @Override
        public void onActivityCreated(@NonNull Activity activity, @Nullable Bundle state) {}

        /** A new Activity starts with Android's default bars, so set them again. */
        @Override
        public void onActivityStarted(@NonNull Activity activity) {
            apply(activity);
        }

        @Override
        public void onActivityResumed(@NonNull Activity activity) {}

        @Override
        public void onActivityPaused(@NonNull Activity activity) {}

        @Override
        public void onActivityStopped(@NonNull Activity activity) {}

        @Override
        public void onActivitySaveInstanceState(@NonNull Activity activity, @NonNull Bundle state) {}

        @Override
        public void onActivityDestroyed(@NonNull Activity activity) {}
    }
}
