/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState } from "react";
import { openDropped } from "../utils/player";
import { formatInvokeError } from "../utils/errors";

/**
 * Desktop: files and folders dropped on the window. Songs play straight
 * away, and each folder becomes a playlist kept in step with it. Returns
 * whether something is being dragged over the window, for the overlay.
 */
export function useFileDrop(
  enabled: boolean,
  onFolder: (folder: string) => Promise<void>,
  setError: (message: string | null) => void,
) {
  const [dragging, setDragging] = useState(false);
  // The handlers change every render; the listener is set up once.
  const latest = useRef({ onFolder, setError });
  latest.current = { onFolder, setError };

  useEffect(() => {
    if (!enabled) return;
    let stop: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      const unlisten = await getCurrentWebview().onDragDropEvent(
        ({ payload }) => {
          if (payload.type === "enter" || payload.type === "over") {
            setDragging(true);
            return;
          }
          setDragging(false);
          if (payload.type !== "drop" || payload.paths.length === 0) return;
          void openDropped(payload.paths)
            .then(async ({ played, folders }) => {
              for (const folder of folders) {
                await latest.current.onFolder(folder);
              }
              if (played === 0 && folders.length === 0) {
                latest.current.setError(
                  "Drop songs to play them, or a folder to make a playlist.",
                );
              }
            })
            .catch((err) =>
              latest.current.setError(
                formatInvokeError(err, "Could not open what was dropped"),
              ),
            );
        },
      );
      if (cancelled) unlisten();
      else stop = unlisten;
    })().catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [enabled]);

  return dragging;
}
