/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import type { ReactNode } from "react";
import { BiX } from "react-icons/bi";

export default function DeviceListPanel({
  onClose,
  children,
}: {
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <div className="right-panel-content">
      <div className="right-panel-header">
        <h2>Devices</h2>
        <button
          className="right-panel-close"
          onClick={onClose}
          type="button"
          title="Close"
        >
          <BiX />
        </button>
      </div>
      <div className="right-panel-list">{children}</div>
    </div>
  );
}
