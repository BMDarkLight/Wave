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
import type { AppUpdate } from "../hooks/useAppUpdate";

/** Announces a release the launch check found, until it is dismissed. */
export default function UpdateToast({ update }: { update: AppUpdate }) {
  if (!update.showToast || !update.latestVersion) return null;
  const ready = update.status === "ready";
  return (
    <div
      className={`update-toast${update.notificationsBlocked ? " update-toast-keep" : ""}`}
      role="status"
      aria-live="polite"
    >
      <span className="update-toast-text">
        {ready
          ? `Wave ${update.latestVersion} is ready to install`
          : `Wave ${update.latestVersion} is out`}
      </span>
      <button
        className="btn-primary"
        type="button"
        onClick={() => {
          if (ready) update.install();
          else update.openPage();
          update.dismissToast();
        }}
      >
        {ready ? "Install" : "Get it"}
      </button>
      <button
        className="update-toast-close"
        type="button"
        aria-label="Dismiss"
        onClick={update.dismissToast}
      >
        <BiX />
      </button>
    </div>
  );
}
