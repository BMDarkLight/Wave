package app.bmdarklight.wave;

import android.util.Log;

import androidx.annotation.Keep;

/**
 * JNI bridge so media notification / audio-focus / headset events can control
 * the Rust audio engine without going through the WebView.
 *
 * When the app is backgrounded, Android may freeze the WebView — JS media
 * handlers then never run. Native dispatch keeps play/pause/next working.
 */
@Keep
public final class MediaNativeBridge {
    private static final String TAG = "MediaNativeBridge";

    private MediaNativeBridge() {}

    /** Called from the media-session plugin (via reflection). */
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

    /** Called from Rust while the WebView is frozen in the background. */
    public static void syncSession(double positionSec, boolean playing) {
        try {
            MediaSessionPlugin.syncSessionFromBackground(positionSec, playing);
        } catch (Throwable t) {
            Log.w(TAG, "syncSession failed: " + t.getMessage());
        }
    }

    private static native void nativeOnMediaAction(String action);
}
