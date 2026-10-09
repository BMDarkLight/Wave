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
 * Tags are written into the file itself, so a preview has nothing to write to.
 *
 * Android `content://` tracks are editable: the backend copies the document
 * out, tags the copy and copies it back. Whether the folder grant actually
 * allows that is a separate question, and one only the backend can answer, so
 * the dialog checks it on open rather than hiding the option here.
 */
export const canEditMetadata = (track: Track) =>
  track.source_state !== "cached";

export const getTrackTitle = (
  track?: Track | null,
  fallbackPath?: string | null,
) => {
  if (track?.title) return track.title;
  if (track?.name) return track.name;
  return fallbackPath ? getFileName(fallbackPath) : "Choose a song";
};

/** Trailing `(Remastered)`, `[2011 Remaster]` and the like. */
const REMASTER_BRACKETS_RE =
  /\s*[([]([^)\]]*\bremaster(?:ed)?\b[^)\]]*)[)\]]\s*$/i;
/** Trailing ` - 2016 Remaster`, ` - Remastered Version`. */
const REMASTER_DASH_RE = /\s+-\s+([^-]*\bremaster(?:ed)?\b[^-]*)$/i;

/**
 * Split a remaster note off the end of a title, so a narrow card can show
 * the song's name in full and the note as a small tag beside it.
 */
export const splitRemasterTag = (
  title: string,
): { name: string; tag: string | null } => {
  const match =
    title.match(REMASTER_BRACKETS_RE) ?? title.match(REMASTER_DASH_RE);
  if (!match || match.index === undefined || match.index === 0) {
    return { name: title, tag: null };
  }
  const note = match[1].trim();
  return {
    name: title.slice(0, match.index).trim(),
    tag: /^remaster(?:ed)?$/i.test(note) ? "Remastered" : note,
  };
};

export const emptyPlaybackState: PlaybackState = {
  is_playing: false,
  is_paused: false,
  current_path: null,
  position_seconds: 0,
  duration_seconds: null,
  volume: 0.8,
  output_device_name: "",
  sleep_timer: { mode: "off" },
  speed: 1,
};
