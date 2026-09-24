/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiX } from "react-icons/bi";
import type { PlaylistInfo } from "../../utils/player";

export default function AddToPlaylistDialog({
  playlists,
  excludePlaylistId,
  onClose,
  onSelect,
}: {
  playlists: PlaylistInfo[];
  excludePlaylistId: string | null;
  onClose: () => void;
  onSelect: (playlistId: string) => void;
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
          {playlists
            .filter((p) => p.id !== excludePlaylistId && p.name !== "Favorites")
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
