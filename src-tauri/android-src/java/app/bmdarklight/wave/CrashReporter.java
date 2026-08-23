package app.bmdarklight.wave;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.widget.Toast;

import androidx.annotation.Keep;

import java.io.File;
import java.io.FileOutputStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Persists the last uncaught Java/Kotlin crash so the next launch can show it
 * in the Wave UI (no adb required). Also records soft failures from media-session
 * code that we catch instead of crashing.
 */
@Keep
public final class CrashReporter {
    private static final String TAG = "WaveCrashReporter";
    private static final String FILE_NAME = "wave_last_crash.txt";
    private static final AtomicBoolean INSTALLED = new AtomicBoolean(false);

    private CrashReporter() {}

    public static void install(Context context) {
        if (context == null || !INSTALLED.compareAndSet(false, true)) {
            return;
        }
        final Context app = context.getApplicationContext();
        final Thread.UncaughtExceptionHandler previous =
                Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((thread, throwable) -> {
            try {
                record(app, "FATAL", throwable);
                toast(app, "Wave crashed — reopen to see the error report");
                // Brief pause so the toast / file flush can land before the process dies.
                try {
                    Thread.sleep(400);
                } catch (InterruptedException ignored) {
                }
            } catch (Throwable ignored) {
            }
            if (previous != null) {
                previous.uncaughtException(thread, throwable);
            } else {
                System.exit(1);
            }
        });
        Log.i(TAG, "Uncaught exception handler installed");
    }

    /** Soft failure: keep running, but persist a report for the UI. */
    public static void recordError(Context context, String where, Throwable error) {
        if (context == null) return;
        try {
            record(context.getApplicationContext(), "ERROR:" + where, error);
            toast(context.getApplicationContext(), "Wave error: " + where);
        } catch (Throwable ignored) {
        }
    }

    public static void recordMessage(Context context, String where, String message) {
        if (context == null) return;
        try {
            String body = "Wave soft failure\n"
                    + "When: " + now() + "\n"
                    + "Where: " + where + "\n"
                    + "Message: " + message + "\n";
            write(context.getApplicationContext(), body);
            Log.e(TAG, where + ": " + message);
            toast(context.getApplicationContext(), "Wave: " + where);
        } catch (Throwable ignored) {
        }
    }

    /** Returns the saved report and deletes the file (one-shot). */
    public static String takeLast(Context context) {
        if (context == null) return null;
        try {
            File file = new File(context.getApplicationContext().getFilesDir(), FILE_NAME);
            if (!file.isFile()) return null;
            byte[] bytes = readAll(file);
            if (bytes == null || bytes.length == 0) return null;
            // Keep a copy until the UI explicitly clears, but prefer one-shot take.
            String text = new String(bytes, StandardCharsets.UTF_8);
            // Don't delete here — clearLast does that after the user dismisses.
            return text;
        } catch (Throwable t) {
            Log.w(TAG, "takeLast failed: " + t.getMessage());
            return null;
        }
    }

    public static void clearLast(Context context) {
        if (context == null) return;
        try {
            File file = new File(context.getApplicationContext().getFilesDir(), FILE_NAME);
            if (file.exists() && !file.delete()) {
                Log.w(TAG, "Failed to delete " + file.getAbsolutePath());
            }
        } catch (Throwable ignored) {
        }
    }

    private static void record(Context app, String kind, Throwable error) {
        StringWriter sw = new StringWriter();
        PrintWriter pw = new PrintWriter(sw);
        pw.println("Wave " + kind);
        pw.println("When: " + now());
        pw.println("Thread: " + Thread.currentThread().getName());
        if (error != null) {
            pw.println("Type: " + error.getClass().getName());
            pw.println("Message: " + error.getMessage());
            pw.println();
            error.printStackTrace(pw);
        }
        pw.flush();
        write(app, sw.toString());
        Log.e(TAG, kind, error);
    }

    private static void write(Context app, String body) {
        File file = new File(app.getFilesDir(), FILE_NAME);
        try (FileOutputStream out = new FileOutputStream(file, false)) {
            out.write(body.getBytes(StandardCharsets.UTF_8));
            out.flush();
        } catch (Throwable t) {
            Log.e(TAG, "write failed: " + t.getMessage());
        }
    }

    private static byte[] readAll(File file) {
        try {
            long len = file.length();
            if (len <= 0 || len > 512_000) return null;
            byte[] buf = new byte[(int) len];
            try (java.io.FileInputStream in = new java.io.FileInputStream(file)) {
                int off = 0;
                while (off < buf.length) {
                    int n = in.read(buf, off, buf.length - off);
                    if (n < 0) break;
                    off += n;
                }
            }
            return buf;
        } catch (Throwable t) {
            return null;
        }
    }

    private static void toast(Context app, String message) {
        try {
            new Handler(Looper.getMainLooper()).post(() -> {
                try {
                    Toast.makeText(app, message, Toast.LENGTH_LONG).show();
                } catch (Throwable ignored) {
                }
            });
        } catch (Throwable ignored) {
        }
    }

    private static String now() {
        return new SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS Z", Locale.US).format(new Date());
    }
}
