package app.bmdarklight.wave;

import android.content.Context;
import android.net.wifi.WifiManager;

import androidx.annotation.Keep;

/**
 * Holds a Wi-Fi multicast lock while the library is shared, so the phone
 * hears the searches other devices send to find media servers. Android drops
 * multicast without one to save power. Needs CHANGE_WIFI_MULTICAST_STATE.
 */
@Keep
public final class WifiMulticast {
    private static WifiManager.MulticastLock lock;

    private WifiMulticast() {}

    /** Takes the lock. True when it is held. */
    public static synchronized boolean acquire(Context context) {
        try {
            if (lock == null) {
                WifiManager wifi = (WifiManager) context.getApplicationContext()
                        .getSystemService(Context.WIFI_SERVICE);
                if (wifi == null) {
                    return false;
                }
                lock = wifi.createMulticastLock("wave-library-sharing");
                lock.setReferenceCounted(false);
            }
            if (!lock.isHeld()) {
                lock.acquire();
            }
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    /** Lets the lock go, if it is held. */
    public static synchronized void release() {
        try {
            if (lock != null && lock.isHeld()) {
                lock.release();
            }
        } catch (Throwable ignored) {
            // Nothing is left to undo.
        }
    }
}
