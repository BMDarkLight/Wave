/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiCloud, BiFolderPlus } from "react-icons/bi";

/** What to add as a media source: a folder on this device or a server. */
export default function AddSourceChooser({
  onLocalFolder,
  onRemoteSource,
  onCancel,
}: {
  onLocalFolder: () => void;
  onRemoteSource: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal-dialog add-source-chooser"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onCancel();
        }}
      >
        <div className="modal-header">
          <h2>Add a media source</h2>
        </div>
        <div className="add-source-options">
          <button
            className="add-source-option"
            type="button"
            onClick={onLocalFolder}
          >
            <BiFolderPlus /> Local folder
          </button>
          <button
            className="add-source-option"
            type="button"
            onClick={onRemoteSource}
          >
            <BiCloud /> Remote source
          </button>
        </div>
        <div className="modal-actions">
          <button className="btn-ghost" onClick={onCancel} type="button">
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
