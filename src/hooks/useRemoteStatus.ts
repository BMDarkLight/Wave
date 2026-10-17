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
  getRemoteStatus,
  listenToRemoteStatus,
  type RemoteStatus,
} from "../utils/remote";

/** Network gate and per-source state, kept current from backend events. */
export function useRemoteStatus(): RemoteStatus | null {
  const [status, setStatus] = useState<RemoteStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      getRemoteStatus()
        .then((next) => {
          if (!cancelled) setStatus(next);
        })
        .catch(() => {});
    };
    load();
    let unlisten: (() => void) | null = null;
    listenToRemoteStatus(load)
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return status;
}
