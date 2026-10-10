/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useBackLayer } from "../../utils/backLayers";

export default function RemoveFromLibraryDialog({
  title,
  onCancel,
  onConfirm,
}: {
  title: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  useBackLayer(true, onCancel);

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal-dialog confirm-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onCancel();
        }}
      >
        <div className="modal-header">
          <h2>Remove from library?</h2>
        </div>
        <p className="confirm-text">
          "{title}" will be removed from your library and every playlist. The
          file itself stays where it is.
        </p>
        <div className="modal-actions">
          <button className="btn-ghost" onClick={onCancel} type="button">
            Cancel
          </button>
          <button className="btn-danger" onClick={onConfirm} type="button">
            Remove
          </button>
        </div>
      </div>
    </div>
  );
}
