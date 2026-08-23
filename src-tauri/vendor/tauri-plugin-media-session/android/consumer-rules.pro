# Proguard rules for consumers of this library.
-keep class app.tauri.mediasession.MediaSessionPlugin {
    public static void syncSessionFromBackground(double, boolean);
}
-keep class app.tauri.mediasession.MediaSessionState { *; }
