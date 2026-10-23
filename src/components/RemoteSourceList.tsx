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
import { BiCloud, BiTrash } from "react-icons/bi";
import { formatRelativeTime } from "../utils/format";
import {
  listRemoteSources,
  listenToLibraryChanged,
  listenToRemoteStatus,
  listenToRemoteSync,
  removeRemoteSource,
  type RemoteSource,
  type RemoteSyncProgress,
} from "../utils/remote";
import RemoteSourceSetupDialog from "./dialogs/RemoteSourceSetupDialog";

function stateLabel(
  source: RemoteSource,
  progress: RemoteSyncProgress | undefined,
): string | null {
  switch (source.state) {
    case "ok":
      return source.last_sync_at
        ? `${source.track_count.toLocaleString()} tracks · synced ${formatRelativeTime(source.last_sync_at)}`
        : `${source.track_count.toLocaleString()} tracks`;
    case "syncing":
      return progress
        ? `Syncing… ${progress.synced.toLocaleString()}/${(progress.total ?? progress.synced).toLocaleString()}`
        : "Syncing…";
    case "unreachable":
      return "Can't reach the server";
    case "blocked":
      return "Waiting for Wi-Fi";
    case "signed_out":
      return null;
  }
}

/** The remote sources Wave knows about, with their state and a remove action. */
export default function RemoteSourceList() {
  const [sources, setSources] = useState<RemoteSource[]>([]);
  const [progress, setProgress] = useState<Record<string, RemoteSyncProgress>>(
    {},
  );
  const [reauthorize, setReauthorize] = useState<RemoteSource | null>(null);
  const [removing, setRemoving] = useState<RemoteSource | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      listRemoteSources()
        .then((next) => {
          if (!cancelled) setSources(next);
        })
        .catch(() => {});
    };
    load();
    const stops: (() => void)[] = [];
    for (const subscribe of [
      listenToRemoteStatus(load),
      listenToLibraryChanged(load),
      listenToRemoteSync((update) =>
        setProgress((current) => ({ ...current, [update.source_id]: update })),
      ),
    ]) {
      void subscribe
        .then((stop) => {
          if (cancelled) stop();
          else stops.push(stop);
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
      stops.forEach((stop) => stop());
    };
  }, []);

  const confirmRemove = async () => {
    if (!removing) return;
    const id = removing.id;
    setRemoving(null);
    try {
      await removeRemoteSource(id);
    } finally {
      setSources((current) => current.filter((source) => source.id !== id));
    }
  };

  if (sources.length === 0) return null;

  return (
    <div className="mset-card mset-folder-list remote-source-list">
      {sources.map((source) => {
        const label = stateLabel(source, progress[source.id]);
        return (
          <div className="mset-row mset-folder-row" key={source.id}>
            <span className="mset-row-text">
              <span className="mset-row-label">
                <BiCloud /> {source.display_name}
              </span>
              {label && <span className="mset-row-value">{label}</span>}
              {source.state === "signed_out" && source.reauth !== "none" && (
                <button
                  className="btn-secondary remote-source-signin"
                  type="button"
                  onClick={() => setReauthorize(source)}
                >
                  Sign in again
                </button>
              )}
            </span>
            <button
              className="mset-icon-btn"
              onClick={() => setRemoving(source)}
              type="button"
              title="Remove this source"
              aria-label="Remove this source"
            >
              <BiTrash />
            </button>
          </div>
        );
      })}

      {reauthorize && (
        <RemoteSourceSetupDialog
          reauthorizeSourceId={reauthorize.id}
          reauthorizeMethod={reauthorize.reauth}
          onClose={() => setReauthorize(null)}
        />
      )}

      {removing && (
        <div className="modal-backdrop" onClick={() => setRemoving(null)}>
          <div
            className="modal-dialog confirm-dialog"
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => {
              if (event.key === "Escape") setRemoving(null);
            }}
          >
            <p className="confirm-text">
              Remove {removing.display_name}? Its offline copies will be
              cleared, but anything you downloaded stays.
            </p>
            <div className="modal-actions">
              <button
                className="btn-ghost"
                onClick={() => setRemoving(null)}
                type="button"
              >
                Cancel
              </button>
              <button
                className="btn-danger"
                onClick={() => void confirmRemove()}
                type="button"
              >
                Remove
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
