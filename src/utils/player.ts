import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface PlaybackState {
  is_playing: boolean;
  is_paused: boolean;
  current_path: string | null;
}

export const playTrack = (path: string): Promise<void> => {
  return invoke("play_track", { path });
};

export const pauseTrack = (): Promise<void> => {
  return invoke("pause_track");
};

export const resumeTrack = (): Promise<void> => {
  return invoke("resume_track");
};

export const stopTrack = (): Promise<void> => {
  return invoke("stop_track");
};

export const getPlaybackState = (): Promise<PlaybackState> => {
  return invoke("get_playback_state");
};

export const selectAudioFile = async (multiple: boolean = false): Promise<string[] | null> => {
  try {
    const selected = await open({
      multiple,
      filters: [
        {
          name: "Audio",
          extensions: ["mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "mka"],
        },
      ],
      title: multiple ? "Select Audio Files" : "Select Audio File",
    });

    if (selected === null) {
      return null;
    }

    if (Array.isArray(selected)) {
      return selected;
    }

    if (typeof selected === "string") {
      return [selected];
    }

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
  return invoke("add_track_to_playlist", { path });
};

export const removeTrackFromPlaylist = (index: number): Promise<void> => {
  return invoke("remove_track_from_playlist", { index });
};

export const getPlaylist = (): Promise<Track[]> => {
  return invoke("get_playlist");
};

export const clearPlaylist = (): Promise<void> => {
  return invoke("clear_playlist");
};

export const playTrackFromPlaylist = (index: number): Promise<void> => {
  return invoke("play_track_from_playlist", { index });
};