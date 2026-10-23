/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import type { SourceSettings } from "../utils/player";
import { getShareStatus, type ShareStatus } from "../utils/remote";
import { isAndroid } from "../utils/platform";

/** How often the status line checks the running server again. */
const STATUS_EVERY_MS = 10_000;

/**
 * The "Share my library" switch under Outside Sourcing, the name other
 * devices see, and whether the server is up and where.
 */
export default function LibrarySharingRow({
  sourceSettings,
  onSourceSettingsChange,
  idPrefix,
}: {
  sourceSettings: SourceSettings;
  onSourceSettingsChange: (next: SourceSettings) => void;
  idPrefix: string;
}) {
  const enabled = sourceSettings.library_sharing_enabled;
  // Kept here while typing: each saved name restarts the server.
  const [name, setName] = useState(sourceSettings.share_name);
  const [status, setStatus] = useState<ShareStatus | null>(null);
  const [onAndroid, setOnAndroid] = useState(false);

  useEffect(() => {
    void isAndroid()
      .then(setOnAndroid)
      .catch(() => {});
  }, []);

  useEffect(
    () => setName(sourceSettings.share_name),
    [sourceSettings.share_name],
  );

  useEffect(() => {
    if (!enabled) {
      setStatus(null);
      return;
    }
    let cancelled = false;
    const load = () => {
      getShareStatus()
        .then((next) => {
          if (!cancelled) setStatus(next);
        })
        .catch(() => {});
    };
    // The server takes a moment to start after the switch is turned on.
    const first = window.setTimeout(load, 800);
    const again = window.setInterval(load, STATUS_EVERY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(first);
      window.clearInterval(again);
    };
  }, [enabled, sourceSettings.share_name]);

  const commitName = () => {
    const trimmed = name.trim();
    if (trimmed !== sourceSettings.share_name) {
      onSourceSettingsChange({ ...sourceSettings, share_name: trimmed });
    }
  };

  return (
    <>
      <label className="mset-gapless-row mset-nested-row">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(event) =>
            onSourceSettingsChange({
              ...sourceSettings,
              library_sharing_enabled: event.target.checked,
            })
          }
        />
        <span className="mset-gapless-copy">
          <span className="mset-gapless-label">Share my library</span>
          <span className="mset-gapless-hint">
            Other devices on your network can browse and play your music, as a
            DLNA or UPnP media server. Only songs stored on this device are
            shared, and nothing outside your network can reach them.
          </span>
        </span>
      </label>
      {enabled && (
        <>
          <div className="mset-key mset-nested-row">
            <label className="mset-key-label" htmlFor={`${idPrefix}share-name`}>
              Name
            </label>
            <input
              id={`${idPrefix}share-name`}
              type="text"
              className="mset-key-input"
              value={name}
              placeholder={status?.name || "Wave"}
              spellCheck={false}
              onChange={(event) => setName(event.target.value)}
              onBlur={commitName}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
            />
          </div>
          <label className="mset-gapless-row mset-nested-row">
            <input
              type="checkbox"
              checked={sourceSettings.share_in_background}
              onChange={(event) =>
                onSourceSettingsChange({
                  ...sourceSettings,
                  share_in_background: event.target.checked,
                })
              }
            />
            <span className="mset-gapless-copy">
              <span className="mset-gapless-label">
                Keep sharing in the background
              </span>
              <span className="mset-gapless-hint">
                {onAndroid
                  ? "Wave keeps running after you leave it, with a notification while it shares."
                  : "Closing the window hides Wave instead of quitting, so players can still reach your music."}
              </span>
            </span>
          </label>
          <p className="mset-hint mset-nested-row share-status">
            {shareStatusText(status)}
          </p>
        </>
      )}
    </>
  );
}

function shareStatusText(status: ShareStatus | null): string {
  if (!status) return "Starting…";
  if (!status.running) {
    return status.error ?? "Not sharing right now.";
  }
  const songs = `${status.track_count.toLocaleString()} ${
    status.track_count === 1 ? "song" : "songs"
  }`;
  const where = status.addresses.length
    ? ` at ${status.addresses.join(", ")}`
    : "";
  const line = `Sharing ${songs} as “${status.name}”${where}.`;
  return status.error ? `${line} ${status.error}.` : line;
}
