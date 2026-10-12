package app.tauri.mediasession

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Manifest-declared receiver for the notification buttons and its dismiss
 * intent. An explicit receiver is the reliable way to get these broadcasts on
 * every API level.
 */
class MediaActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.getStringExtra(EXTRA_ACTION) ?: return
        Log.d(TAG, "onReceive: action=\"$action\"")
        MediaSessionState.init(context)

        if (!MediaSessionState.hasMedia) {
            // Left over from a process that has since died, so its buttons can
            // no longer reach a player. Clear it instead of leaving dead controls.
            MediaSessionCleanupService.cancel(context)
            return
        }
        if (action == "dismiss") {
            // Take the controls down here and now, on the main thread. The
            // engine may end the process right after it hears about this, and
            // a queued update would never get to remove the notification.
            MediaSessionState.stopNow()
        }
        MediaSessionPlugin.handleMediaAction(action)
    }

    companion object {
        internal const val EXTRA_ACTION = "app.tauri.mediasession.EXTRA_ACTION"
        private const val TAG = "plugin/media-session"
    }
}
