import { getFileName, type PlaybackState, type Track } from "./player";

export const LIBRARY_PLAYLIST_NAME = "Library";

export const isLibraryPlaylistName = (name?: string | null) =>
  name === LIBRARY_PLAYLIST_NAME || name === "All Local Files";

export const getTrackTitle = (
  track?: Track | null,
  fallbackPath?: string | null,
) => {
  if (track?.title) return track.title;
  if (track?.name) return track.name;
  return fallbackPath ? getFileName(fallbackPath) : "Choose a song";
};

export const emptyPlaybackState: PlaybackState = {
  is_playing: false,
  is_paused: false,
  current_path: null,
  position_seconds: 0,
  duration_seconds: null,
  volume: 0.8,
  output_device_name: "",
};
