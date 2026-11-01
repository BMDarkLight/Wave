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
import {
  installOnlinePlay,
  listenOnlinePlayInstallProgress,
  type OnlinePlayInstallProgress,
  type OnlinePlayStatus,
} from "../../utils/player";

const megabytes = (bytes: number) => `${Math.round(bytes / 1_000_000)} MB`;

const toolName = (tool: OnlinePlayInstallProgress["tool"]) =>
  tool === "deno" ? "Deno" : "yt-dlp";

/**
 * Asks before Online play downloads yt-dlp and Deno, then shows the download.
 * Nothing is fetched until the user agrees; cancelling leaves the switch off.
 */
export default function OnlinePlayInstallDialog({
  onCancel,
  onInstalled,
}: {
  onCancel: () => void;
  onInstalled: (status: OnlinePlayStatus) => void;
}) {
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<OnlinePlayInstallProgress | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!installing) return;
    let stop: (() => void) | undefined;
    let gone = false;
    void listenOnlinePlayInstallProgress(setProgress).then((unlisten) => {
      if (gone) unlisten();
      else stop = unlisten;
    });
    return () => {
      gone = true;
      stop?.();
    };
  }, [installing]);

  const install = async () => {
    setError(null);
    setProgress(null);
    setInstalling(true);
    try {
      onInstalled(await installOnlinePlay());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setInstalling(false);
    }
  };

  const fraction =
    progress && progress.total > 0
      ? Math.min(progress.received / progress.total, 1)
      : 0;

  return (
    <div className="modal-backdrop" onClick={installing ? undefined : onCancel}>
      <div
        className="modal-dialog confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="online-play-install-title"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !installing) onCancel();
        }}
      >
        <div className="modal-header">
          <h2 id="online-play-install-title">
            {installing ? "Installing Online play" : "Install Online play?"}
          </h2>
        </div>
        {installing ? (
          <>
            <p className="confirm-text" aria-live="polite">
              {progress
                ? `Downloading ${toolName(progress.tool)}… ${megabytes(progress.received)} of ${megabytes(progress.total)}`
                : "Getting ready…"}
            </p>
            <div
              className={`import-progress-bar${progress ? "" : " indeterminate"}`}
              role="progressbar"
              aria-label="Download progress"
              aria-valuemin={0}
              aria-valuemax={progress?.total || undefined}
              aria-valuenow={progress?.received}
            >
              <span style={{ width: `${fraction * 100}%` }} />
            </div>
          </>
        ) : (
          <>
            <p className="confirm-text">
              Online play finds the full song on YouTube when you play a catalog
              result. It needs two programs Wave doesn't include: yt-dlp, and
              the Deno runtime it uses to talk to YouTube.
            </p>
            <p className="confirm-text">
              Wave will download them from their official GitHub releases (about
              80 MB), check each against its published checksum, and keep them
              up to date. You can remove them from Settings at any time.
            </p>
            <p className="confirm-text">
              If YouTube can&apos;t be reached from this computer, Wave tries
              your other paired Waves, then public Invidious and Piped servers.
              Those servers see which songs you play.
            </p>
            {error && (
              <p className="confirm-text" role="alert">
                {error}
              </p>
            )}
          </>
        )}
        <div className="modal-actions">
          <button
            className="btn-ghost"
            onClick={onCancel}
            type="button"
            disabled={installing}
          >
            {error ? "Close" : "Not now"}
          </button>
          <button
            className="btn-primary"
            onClick={() => void install()}
            type="button"
            disabled={installing}
          >
            {error ? "Try again" : "Install"}
          </button>
        </div>
      </div>
    </div>
  );
}
