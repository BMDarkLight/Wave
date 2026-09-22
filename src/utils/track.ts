/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { getFileName, type PlaybackState, type Track } from "./player";

export const LIBRARY_PLAYLIST_NAME = "Library";

export const isLibraryPlaylistName = (name?: string | null) =>
  name === LIBRARY_PLAYLIST_NAME || name === "All Local Files";

/**
 * Tags are written into the file itself, which rules out previews and the
 * `content://` URIs that Android's folder picker hands back.
 */
export const canEditMetadata = (track: Track) =>
  !track.path.startsWith("content://") && track.source_state !== "cached";

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
