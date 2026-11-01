/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiAlbum, BiCloudDownload, BiPlay, BiUser } from "react-icons/bi";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";
import type { SourceTrack } from "../utils/player";

/** The menu of a result from an online catalog. */
export default function SourceTrackMenu({
  track,
  anchor,
  onClose,
  onPlay,
  onDownload,
  onGoToArtist,
  onGoToAlbum,
}: {
  track: SourceTrack;
  anchor: ContextMenuAnchor;
  onClose: () => void;
  onPlay: (track: SourceTrack) => void;
  onDownload: (track: SourceTrack) => void;
  onGoToArtist: (provider: string, id: string, name: string) => void;
  onGoToAlbum: (provider: string, id: string, title: string) => void;
}) {
  const artistRef = track.artist_ref;
  const albumRef = track.album_ref;
  return (
    <ContextMenu anchor={anchor} onClose={onClose}>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
          onPlay(track);
        }}
      >
        <BiPlay /> Play
      </button>
      {track.downloadable && !track.already_in_library && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onDownload(track);
          }}
        >
          <BiCloudDownload /> Save to Library
        </button>
      )}
      {albumRef && track.album && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onGoToAlbum(track.provider, albumRef, track.album ?? "");
          }}
        >
          <BiAlbum /> Go to Album
        </button>
      )}
      {artistRef && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onGoToArtist(track.provider, artistRef, track.artist);
          }}
        >
          <BiUser /> Go to Artist
        </button>
      )}
    </ContextMenu>
  );
}
