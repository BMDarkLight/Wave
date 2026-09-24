/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiFolderOpen, BiPlus } from "react-icons/bi";
import ContextMenu from "./ContextMenu";

export default function AddTrackMenu({
  anchor,
  onClose,
  androidHost,
  onAddFiles,
  onAddFolder,
  onAddFolderAsPlaylist,
}: {
  anchor: { top: number; left: number };
  onClose: () => void;
  androidHost: boolean;
  onAddFiles: () => void;
  onAddFolder: () => void;
  onAddFolderAsPlaylist: () => void;
}) {
  return (
    <ContextMenu anchor={anchor} onClose={onClose} className="add-track-menu">
      <button type="button" onClick={onAddFiles}>
        <BiPlus /> Add files
      </button>
      {!androidHost && (
        <>
          <button type="button" onClick={onAddFolder}>
            <BiFolderOpen /> Add folder
          </button>
          <button type="button" onClick={onAddFolderAsPlaylist}>
            <BiFolderOpen /> Add folder as playlist
          </button>
        </>
      )}
      {androidHost && (
        <p className="add-track-menu-hint">
          On Android, tap the + button to scan a music folder into Library.
        </p>
      )}
    </ContextMenu>
  );
}
