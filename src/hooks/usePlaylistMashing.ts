/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import {
  getAlbumTracks,
  getPlaylistMashingEnabled,
  getPlaylistTracksById,
  playMashup,
  reshuffleMashup,
  setPlaylistMashingEnabled,
  type Track,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";

/** An album or playlist that can go into a mashup. */
export type MashSource =
  | { kind: "playlist"; id: string; name: string }
  | { kind: "album"; name: string; albumArtist: string | null };

export const mashSourceKey = (source: MashSource) =>
  source.kind === "playlist"
    ? `playlist:${source.id}`
    : `album:${source.albumArtist ?? ""}\u0000${source.name}`;

const tracksOf = (source: MashSource): Promise<Track[]> =>
  source.kind === "playlist"
    ? getPlaylistTracksById(source.id)
    : getAlbumTracks(source.name, source.albumArtist);

/** Playlist mashing: the settings switch, playing a mashup of albums and
 * playlists, and shuffling it again. `afterPlay` reloads what the player
 * shows once the queue has changed. */
export function usePlaylistMashing(
  setError: (message: string | null) => void,
  afterPlay: () => Promise<void>,
) {
  const [playlistMashingEnabled, setEnabled] = useState(false);

  useEffect(() => {
    getPlaylistMashingEnabled()
      .then(setEnabled)
      .catch(() => {});
  }, []);

  const handlePlaylistMashingChange = async (enabled: boolean) => {
    try {
      await setPlaylistMashingEnabled(enabled);
      setEnabled(enabled);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to change playlist mashing"));
    }
  };

  const handleMash = async (sources: MashSource[]) => {
    try {
      setError(null);
      const lists = await Promise.all(sources.map(tracksOf));
      // A song on two of them plays once.
      const paths = [...new Set(lists.flat().map((track) => track.path))];
      if (paths.length === 0) {
        setError("There are no songs to mash together");
        return;
      }
      await playMashup(paths);
      await afterPlay();
    } catch (err) {
      setError(formatInvokeError(err, "Could not start the mashup"));
    }
  };

  const handleReshuffleMashup = async () => {
    try {
      await reshuffleMashup();
      await afterPlay();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to reshuffle the mashup"));
    }
  };

  return {
    playlistMashingEnabled,
    handlePlaylistMashingChange,
    handleMash,
    handleReshuffleMashup,
  };
}
