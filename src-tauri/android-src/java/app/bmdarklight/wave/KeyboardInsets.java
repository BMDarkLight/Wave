package app.bmdarklight.wave;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.os.Bundle;
import android.view.View;

import androidx.annotation.Keep;
import androidx.annotation.NonNull;
import androidx.annotation.Nullable;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Shrinks the web view above the soft keyboard.
 *
 * The generated MainActivity draws edge to edge, and in that mode Android no
 * longer resizes the window for the keyboard, so it covers the bottom of the
 * page and anything focused there. This pads the content view by the part of
 * the keyboard that rises above the navigation bar, which the page already
 * keeps clear through its safe area padding. The web view shrinks the way
 * adjustResize would and 100dvh follows it.
 */
@Keep
public final class KeyboardInsets {
    private static final AtomicBoolean INSTALLED = new AtomicBoolean(false);

    private KeyboardInsets() {}

    public static void install(Context context) {
        if (context == null || !INSTALLED.compareAndSet(false, true)) {
            return;
        }
        Context app = context.getApplicationContext();
        if (!(app instanceof Application)) {
            return;
        }
        ((Application) app).registerActivityLifecycleCallbacks(new Callbacks());
    }

    private static void attach(Activity activity) {
        View content = activity.findViewById(android.R.id.content);
        if (content == null) {
            return;
        }
        // Setting it again on a later start only replaces the same listener.
        ViewCompat.setOnApplyWindowInsetsListener(content, (view, insets) -> {
            Insets ime = insets.getInsets(WindowInsetsCompat.Type.ime());
            Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            int bottom = Math.max(0, ime.bottom - bars.bottom);
            if (view.getPaddingBottom() != bottom) {
                view.setPadding(
                        view.getPaddingLeft(),
                        view.getPaddingTop(),
                        view.getPaddingRight(),
                        bottom);
            }
            // Left unconsumed so the web view still gets its safe area insets.
            return insets;
        });
        ViewCompat.requestApplyInsets(content);
    }

    private static final class Callbacks implements Application.ActivityLifecycleCallbacks {
        @Override
        public void onActivityCreated(@NonNull Activity activity, @Nullable Bundle state) {}

        // Started rather than created: by now the activity has set its content
        // view, so looking it up does not install the window decor early.
        @Override
        public void onActivityStarted(@NonNull Activity activity) {
            attach(activity);
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
