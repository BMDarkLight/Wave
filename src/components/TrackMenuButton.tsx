/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiDotsHorizontalRounded } from "react-icons/bi";
import type { ContextMenuAnchor } from "./ContextMenu";
import type { Track } from "../utils/player";

/**
 * Trailing cell of a track row that opens the track menu.
 *
 * It sits on hover on desktop and stays put on narrow layouts, which is the
 * only way to reach the menu without a right mouse button.
 */
export default function TrackMenuButton({
  track,
  isOpen,
  onOpen,
  onClose,
}: {
  track: Track;
  isOpen: boolean;
  onOpen: (track: Track, anchor: ContextMenuAnchor) => void;
  onClose: () => void;
}) {
  return (
    <div className="track-actions-cell">
      <div className="track-actions-hover">
        <button
          className="track-action-btn"
          onClick={(event) => {
            event.stopPropagation();
            if (isOpen) {
              onClose();
              return;
            }
            const rect = event.currentTarget.getBoundingClientRect();
            onOpen(track, {
              top: rect.bottom + 4,
              flipAbove: rect.top - 4,
              right: window.innerWidth - rect.right,
            });
          }}
          title="More"
          type="button"
        >
          <BiDotsHorizontalRounded />
        </button>
      </div>
    </div>
  );
}
