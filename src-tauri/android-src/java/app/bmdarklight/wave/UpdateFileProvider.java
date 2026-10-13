package app.bmdarklight.wave;

import androidx.core.content.FileProvider;

/**
 * Shares downloaded update APKs with the system installer. A subclass, so it
 * never clashes with a FileProvider the Tauri template or a plugin declares.
 */
public final class UpdateFileProvider extends FileProvider {}
