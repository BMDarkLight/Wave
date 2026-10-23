# Keep JNI entry points visible to native code (R8 cannot see JNI callers).
-keep class app.bmdarklight.wave.FolderPickerCallback { *; }
-keepclassmembers class app.bmdarklight.wave.FolderPickerCallback {
    public static java.lang.String[] pickForJni(android.app.Activity);
    public static *** pick(android.app.Activity);
}
-keep class app.bmdarklight.wave.FolderPickerCallback$* { *; }
-keep class app.bmdarklight.wave.SafMediaScanner { *; }
-keepclassmembers class app.bmdarklight.wave.SafMediaScanner {
    public static java.lang.String[] listAudioFiles(android.content.Context, java.lang.String);
}
-keep class app.bmdarklight.wave.CrashReporter { *; }
-keepclassmembers class app.bmdarklight.wave.CrashReporter {
    public static void install(android.content.Context);
    public static void recordError(android.content.Context, java.lang.String, java.lang.Throwable);
    public static void recordMessage(android.content.Context, java.lang.String, java.lang.String);
    public static java.lang.String takeLast(android.content.Context);
    public static void clearLast(android.content.Context);
}
-keep class app.bmdarklight.wave.CrashInitProvider { *; }
-keep class app.bmdarklight.wave.KeyboardInsets { *; }
-keep class app.bmdarklight.wave.UiLifecycle { *; }
-keep class app.bmdarklight.wave.SystemTheme { *; }
-keepclassmembers class app.bmdarklight.wave.SystemTheme {
    public static boolean isDark(android.content.Context);
    public static void setLightBars(android.app.Activity, boolean);
}
-keep class app.bmdarklight.wave.NetworkState { *; }
-keepclassmembers class app.bmdarklight.wave.NetworkState {
    public static boolean isCellularOrMetered(android.content.Context);
}
-keep class app.bmdarklight.wave.WifiMulticast { *; }
-keepclassmembers class app.bmdarklight.wave.WifiMulticast {
    public static boolean acquire(android.content.Context);
    public static void release();
}
-keep class app.bmdarklight.wave.UpdateNotifier { *; }
-keepclassmembers class app.bmdarklight.wave.UpdateNotifier {
    public static void progress(android.content.Context, java.lang.String, long, long);
    public static void ready(android.content.Context, java.lang.String, java.lang.String);
    public static void failed(android.content.Context, java.lang.String, java.lang.String);
    public static void cancel(android.content.Context);
    public static boolean enabled(android.content.Context);
}
-keep class app.bmdarklight.wave.ShareService { *; }
-keepclassmembers class app.bmdarklight.wave.ShareService {
    public static void start(android.content.Context, java.lang.String);
    public static void stop(android.content.Context);
}
-keep class app.bmdarklight.wave.SafFile { *; }
-keepclassmembers class app.bmdarklight.wave.SafFile {
    public static int openForReading(android.content.Context, java.lang.String);
}
-keep class app.bmdarklight.wave.AppUpdater { *; }
-keepclassmembers class app.bmdarklight.wave.AppUpdater {
    public static void install(android.app.Activity, java.lang.String);
    public static void openUrl(android.app.Activity, java.lang.String);
}
# UiLifecycle rebinds Tauri's PluginManager to a new Activity by field name.
-keepclassmembers class app.tauri.plugin.PluginManager { *; }
-keep class app.bmdarklight.wave.MediaNativeBridge { *; }
-keepclassmembers class app.bmdarklight.wave.MediaNativeBridge {
    public static void dispatch(java.lang.String);
    public static void attach(android.content.Context);
    public static void updateSession(java.lang.String);
    public static void updateTimeline(double, double, double);
    public static void clearSession();
    public static void setHeadless(boolean);
    public static void shutdownSessionNow();
    private static native void nativeOnMediaAction(java.lang.String);
}
-keep class app.bmdarklight.wave.MediaMetadataProbe { *; }
-keepclassmembers class app.bmdarklight.wave.MediaMetadataProbe {
    public static java.lang.String probe(android.content.Context, java.lang.String);
}
-keep class app.bmdarklight.wave.audio.WaveExoPlayer { *; }
-keep class app.bmdarklight.wave.audio.PeakAnalyzer { *; }
-keepclassmembers class app.bmdarklight.wave.audio.WaveExoPlayer {
    public static app.bmdarklight.wave.audio.WaveExoPlayer getOrCreate(android.content.Context);
    public static float[] analyzeLevels(android.content.Context, java.lang.String);
    public static boolean isPlayingActive();
    public static void setRemoteStreamBase(java.lang.String);
    public void playUri(java.lang.String);
    public void playMediaItems(java.lang.String[], int);
    public void prepareUriAt(java.lang.String, long);
    public void play();
    public void pause();
    public void stop();
    public void seekTo(long);
    public void setVolume(float);
    public void setEqBands(float[]);
    public void setEqEnabled(boolean);
    public void setCrossfadeDuration(float);
    public void setGaplessEnabled(boolean);
    public void setUpcomingUri(java.lang.String);
    public long getCurrentPosition();
    public long getDuration();
    public long positionFor(java.lang.String);
    public long durationFor(java.lang.String);
    public boolean isPlaying();
    public boolean isEnded();
    public boolean isBuffering();
}
