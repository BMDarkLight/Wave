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

export default function DeviceListPanel({
  devices,
  currentDeviceName,
  onClose,
  onSelectDevice,
}: {
  devices: string[];
  currentDeviceName: string;
  onClose: () => void;
  onSelectDevice: (name: string) => void;
}) {
  return (
    <div className="right-panel-content">
      <div className="right-panel-header">
        <h2>Audio Output</h2>
        <button
          className="right-panel-close"
          onClick={onClose}
          type="button"
          title="Close"
        >
          <BiX />
        </button>
      </div>
      <div className="right-panel-list">
        {devices.map((name) => (
          <button
            key={name}
            className={`device-panel-item ${name === currentDeviceName ? "active" : ""}`}
            onClick={() => onSelectDevice(name)}
            type="button"
          >
            {name}
          </button>
        ))}
      </div>
    </div>
  );
}
