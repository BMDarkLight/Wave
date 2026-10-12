package app.bmdarklight.wave;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.os.Bundle;
import android.util.Log;

import androidx.annotation.Keep;
import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Lets the UI come back after Android destroyed it while music kept playing.
 *
 * <p>When the task is swiped away during playback the process stays alive
 * without an Activity. Opening Wave again starts a new Activity in that same
 * process, which Tauri does not expect:
 *
 * <ul>
 *   <li>Tauri only builds windows at launch. {@link #onActivityStarted} tells
 *       the engine an Activity is up so it can build the main window again.</li>
 *   <li>Tauri's PluginManager binds activity-result launchers (file dialogs,
 *       permission prompts) to the first Activity and ignores later ones.
 *       {@link #onActivityCreated} unbinds it from a destroyed Activity, so
 *       the new one registers fresh launchers when it finishes onCreate.</li>
 * </ul>
 */
@Keep
public final class UiLifecycle {
    private static final String TAG = "WaveUiLifecycle";
    private static final AtomicBoolean INSTALLED = new AtomicBoolean(false);

    private UiLifecycle() {}

    public static void install(Context context) {
        if (!INSTALLED.compareAndSet(false, true)) return;
        Context app = context.getApplicationContext();
        if (app instanceof Application) {
            ((Application) app).registerActivityLifecycleCallbacks(new Callbacks());
        }
    }

    /**
     * Runs inside the new Activity's super.onCreate, before TauriActivity
     * hands it to PluginManager.onActivityCreate. That method returns early
     * while its activity field is set, so clear the field when it still points
     * at a destroyed Activity.
     */
    private static void releaseStalePluginActivity() {
        try {
            Class<?> cls = Class.forName("app.tauri.plugin.PluginManager");
            Object instance = cls.getField("INSTANCE").get(null);
            Field field = cls.getDeclaredField("activity");
            field.setAccessible(true);
            Object owner = Modifier.isStatic(field.getModifiers()) ? null : instance;
            Object current = field.get(owner);
            if (current instanceof Activity && ((Activity) current).isDestroyed()) {
                field.set(owner, null);
                Log.i(TAG, "Released Tauri PluginManager from a destroyed Activity");
            }
        } catch (Throwable t) {
            Log.w(TAG, "Could not rebind Tauri PluginManager: " + t);
        }
    }

    private static final class Callbacks implements Application.ActivityLifecycleCallbacks {
        @Override
        public void onActivityCreated(@NonNull Activity activity, @Nullable Bundle state) {
            releaseStalePluginActivity();
        }

        @Override
        public void onActivityStarted(@NonNull Activity activity) {
            // Ignored by the engine unless the UI had been destroyed.
            MediaNativeBridge.dispatch("ui-started");
        }

        @Override
        public void onActivityResumed(@NonNull Activity activity) {}

        @Override
        public void onActivityPaused(@NonNull Activity activity) {}

        @Override
        public void onActivityStopped(@NonNull Activity activity) {}

        @Override
        public void onActivitySaveInstanceState(@NonNull Activity activity, @NonNull Bundle outState) {}

        @Override
        public void onActivityDestroyed(@NonNull Activity activity) {}
    }
}
