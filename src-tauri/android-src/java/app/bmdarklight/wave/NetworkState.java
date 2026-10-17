package app.bmdarklight.wave;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;

import androidx.annotation.Keep;

/**
 * Whether the phone is on mobile data, so remote music can stay off it when
 * the user asks. Needs ACCESS_NETWORK_STATE.
 */
@Keep
public final class NetworkState {
    private NetworkState() {}

    /**
     * True on cellular, or on a metered network that is not Wi-Fi or
     * Ethernet (a tethered hotspot, for one). False when it can't tell.
     */
    public static boolean isCellularOrMetered(Context context) {
        try {
            ConnectivityManager cm =
                    (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (cm == null) {
                return false;
            }
            Network network = cm.getActiveNetwork();
            if (network == null) {
                return false;
            }
            NetworkCapabilities caps = cm.getNetworkCapabilities(network);
            if (caps == null) {
                return false;
            }
            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
                return true;
            }
            boolean metered = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED);
            return metered
                    && !caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                    && !caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET);
        } catch (Throwable t) {
            return false;
        }
    }
}
