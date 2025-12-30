// Use dynamic imports to handle cases where Tauri API might not be available
let invokeFn: typeof import("@tauri-apps/api/core").invoke;
let openFn: typeof import("@tauri-apps/plugin-dialog").open;

// Initialize Tauri APIs
const initTauri = async () => {
  try {
    const core = await import("@tauri-apps/api/core");
    const dialog = await import("@tauri-apps/plugin-dialog");
    invokeFn = core.invoke;
    openFn = dialog.open;
    console.log("✓ Tauri APIs initialized");
    return true;
  } catch (error) {
    console.error("✗ Failed to initialize Tauri APIs:", error);
    return false;
  }
};

// Initialize immediately
let tauriInitialized = initTauri();

// Wrapper for invoke that ensures Tauri is initialized
const safeInvoke = async <T = any>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  // Wait for Tauri to be initialized
  await tauriInitialized;
  
  if (!invokeFn) {
    throw new Error("Tauri API is not available. Make sure you're running 'npm run tauri dev' (not just 'npm run dev')");
  }
  
  try {
    return await invokeFn<T>(cmd, args);
  } catch (error: any) {
    // Re-throw errors as-is
    throw error;
  }
};

export interface PlaybackState {
  is_playing: boolean;
  is_paused: boolean;
  current_path: string | null;
}

export const playTrack = (path: string): Promise<void> => {
  console.log("playTrack called with path:", path);
  return safeInvoke("play_track", { path });
};

export const pauseTrack = (): Promise<void> => {
  return safeInvoke("pause_track");
};

export const resumeTrack = (): Promise<void> => {
  return safeInvoke("resume_track");
};

export const stopTrack = (): Promise<void> => {
  return safeInvoke("stop_track");
};

export const getPlaybackState = (): Promise<PlaybackState> => {
  return safeInvoke<PlaybackState>("get_playback_state");
};

export const selectAudioFile = async (multiple: boolean = false): Promise<string[] | null> => {
  console.log("selectAudioFile called", { multiple });
  
  // Wait for Tauri to be initialized
  await tauriInitialized;
  
  if (!openFn) {
    throw new Error("Tauri API is not available. Make sure you're running 'npm run tauri dev' (not just 'npm run dev')");
  }
  
  try {
    console.log("Calling open dialog...");
    const selected = await openFn({
      multiple,
      filters: [
        {
          name: "Audio",
          extensions: ["mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "mka"],
        },
      ],
      title: multiple ? "Select Audio Files" : "Select Audio File",
    });
    console.log("Dialog returned:", selected);

    if (selected === null) {
      console.log("User cancelled file selection");
      return null;
    }

    if (Array.isArray(selected)) {
      console.log("Multiple files selected:", selected.length);
      return selected;
    }

    if (typeof selected === "string") {
      console.log("Single file selected:", selected);
      return [selected];
    }

    console.warn("Unexpected return type from dialog:", typeof selected);
    return null;
  } catch (error) {
    console.error("Error selecting file:", error);
    throw new Error(error instanceof Error ? error.message : "Failed to open file dialog");
  }
};

export const getFileName = (path: string | null): string => {
  if (!path) return "No track selected";
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || "Unknown";
};

export interface Track {
  path: string;
  name: string;
}

export const addTrackToPlaylist = (path: string): Promise<Track> => {
  console.log("addTrackToPlaylist called with path:", path);
  return safeInvoke<Track>("add_track_to_playlist", { path });
};

export const removeTrackFromPlaylist = (index: number): Promise<void> => {
  return safeInvoke("remove_track_from_playlist", { index });
};

export const getPlaylist = (): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_playlist");
};

export const clearPlaylist = (): Promise<void> => {
  return safeInvoke("clear_playlist");
};

export const playTrackFromPlaylist = (index: number): Promise<void> => {
  return safeInvoke("play_track_from_playlist", { index });
};