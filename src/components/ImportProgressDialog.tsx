/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import type { ImportProgress } from "../hooks/useMediaImport";

const count = new Intl.NumberFormat();

/**
 * Follows the first import after the Welcome dialog: finding the songs in the
 * picked folder, adding them, then done. Hiding it leaves the import running.
 */
export default function ImportProgressDialog({
  progress,
  onClose,
}: {
  progress: ImportProgress;
  onClose: () => void;
}) {
  const { phase, processed, total, added } = progress;
  const fraction = total > 0 ? Math.min(processed / total, 1) : 0;

  let title: string;
  let detail: string;
  if (phase === "finding") {
    title = "Finding your music";
    detail = "Looking through the folder you picked…";
  } else if (phase === "adding") {
    title = "Adding songs to your Library";
    detail =
      total > 0
        ? `${count.format(processed)} of ${count.format(total)}`
        : "Getting ready…";
  } else {
    title = "Your Library is ready";
    detail =
      added === 1
        ? "1 song added."
        : added > 0
          ? `${count.format(added)} songs added.`
          : "Everything in that folder is already in your Library.";
  }

  return (
    <div className="modal-backdrop">
      <div
        className="modal-dialog import-progress-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="import-progress-title"
      >
        <h2 id="import-progress-title">{title}</h2>
        <p className="modal-desc" aria-live="polite">
          {detail}
        </p>
        {phase !== "done" && (
          <div
            className={`import-progress-bar${phase === "finding" || total === 0 ? " indeterminate" : ""}`}
            role="progressbar"
            aria-label={title}
            aria-valuemin={0}
            aria-valuemax={total || undefined}
            aria-valuenow={total ? processed : undefined}
          >
            <span style={{ width: `${fraction * 100}%` }} />
          </div>
        )}
        <div className="modal-actions">
          {phase === "done" ? (
            <button className="btn-primary" type="button" onClick={onClose}>
              Start listening
            </button>
          ) : (
            <button className="btn-ghost" type="button" onClick={onClose}>
              Continue in background
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
