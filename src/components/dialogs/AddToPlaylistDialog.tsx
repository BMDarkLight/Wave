/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiPlus, BiX } from "react-icons/bi";
import type { PlaylistInfo } from "../../utils/player";
import { isLibraryPlaylistName } from "../../utils/track";

export default function AddToPlaylistDialog({
  playlists,
  excludePlaylistId,
  onClose,
  onSelect,
  onCreateNew,
}: {
  playlists: PlaylistInfo[];
  excludePlaylistId: string | null;
  onClose: () => void;
  onSelect: (playlistId: string) => void;
  onCreateNew: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>Add to playlist</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>
        <div className="playlist-picker-list">
          <button
            className="playlist-picker-item playlist-picker-new"
            type="button"
            onClick={onCreateNew}
          >
            <BiPlus aria-hidden /> New playlist
          </button>
          {playlists
            .filter(
              (p) =>
                p.id !== excludePlaylistId &&
                p.name !== "Favorites" &&
                !isLibraryPlaylistName(p.name),
            )
            .map((p) => (
              <button
                key={p.id}
                className="playlist-picker-item"
                type="button"
                title={p.name}
                onClick={() => onSelect(p.id)}
              >
                {p.name}
              </button>
            ))}
        </div>
      </div>
    </div>
  );
}
