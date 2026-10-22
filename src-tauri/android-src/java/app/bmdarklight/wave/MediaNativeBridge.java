package app.bmdarklight.wave;

import android.content.Context;
import android.util.Log;

import androidx.annotation.Keep;

import app.tauri.mediasession.MediaSessionBridge;

/**
 * JNI bridge between the Rust engine and the Android media session.
 *
 * Inbound: notification, Bluetooth, headset and audio-focus events reach the
 * engine through {@link #dispatch} without going through the WebView, which
 * Android freezes in the background (or destroys when the task is swiped).
 *
 * Outbound: the engine pushes now-playing state into the session with
 * {@link #updateSession}, {@link #updateTimeline} and {@link #clearSession}.
 * These do not need an Activity, unlike Tauri plugin commands.
 */
@Keep
public final class MediaNativeBridge {
    private static final String TAG = "MediaNativeBridge";

    private MediaNativeBridge() {}

    /** Called from the media-session plugin (via reflection) and the player. */
    public static void dispatch(String action) {
        if (action == null || action.isEmpty()) return;
        try {
            nativeOnMediaAction(action);
        } catch (UnsatisfiedLinkError e) {
            Log.w(TAG, "nativeOnMediaAction not registered yet: " + e.getMessage());
        } catch (Throwable t) {
            Log.e(TAG, "dispatch failed for action=" + action, t);
        }
    }

    public static void attach(Context context) {
        try {
            MediaSessionBridge.init(context);
        } catch (Throwable t) {
            Log.w(TAG, "attach failed: " + t.getMessage());
        }
    }

    public static void updateSession(String json) {
        try {
            MediaSessionBridge.update(json);
        } catch (Throwable t) {
            Log.w(TAG, "updateSession failed: " + t.getMessage());
        }
    }

    /** NaN means "unchanged" for any argument. */
    public static void updateTimeline(double positionSec, double durationSec, double speed) {
        try {
            MediaSessionBridge.timeline(positionSec, durationSec, speed);
        } catch (Throwable t) {
            Log.w(TAG, "updateTimeline failed: " + t.getMessage());
        }
    }

    public static void clearSession() {
        try {
            MediaSessionBridge.clear();
        } catch (Throwable t) {
            Log.w(TAG, "clearSession failed: " + t.getMessage());
        }
    }

    /** The UI was destroyed while playback kept the process alive, or is back. */
    public static void setHeadless(boolean headless) {
        try {
            MediaSessionBridge.setHeadless(headless);
        } catch (Throwable t) {
            Log.w(TAG, "setHeadless failed: " + t.getMessage());
        }
    }

    /** Remove the notification and stop the service synchronously, before exit. */
    public static void shutdownSessionNow() {
        try {
            MediaSessionBridge.shutdownNow();
        } catch (Throwable t) {
            Log.w(TAG, "shutdownSessionNow failed: " + t.getMessage());
        }
    }

    private static native void nativeOnMediaAction(String action);
}
