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
  coupleSongs,
  getSongCoupling,
  setSongCouplingEnabled,
  uncoupleSong,
  type SongCoupling,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";

/** Song coupling: the settings switch and the linked pairs. Every change can
 * rearrange the queue, so the queue is reloaded after each one. */
export function useSongCoupling(
  setError: (message: string | null) => void,
  reloadQueue: () => Promise<void>,
) {
  const [songCoupling, setSongCoupling] = useState<SongCoupling>({
    enabled: false,
    couples: [],
  });

  useEffect(() => {
    getSongCoupling()
      .then(setSongCoupling)
      .catch(() => {});
  }, []);

  const apply = async (change: Promise<SongCoupling>, failure: string) => {
    try {
      setSongCoupling(await change);
      await reloadQueue();
    } catch (err) {
      setError(formatInvokeError(err, failure));
    }
  };

  return {
    songCoupling,
    handleSongCouplingChange: (enabled: boolean) =>
      apply(setSongCouplingEnabled(enabled), "Failed to change song coupling"),
    handleCoupleSongs: (first: string, second: string) =>
      apply(coupleSongs(first, second), "Failed to couple songs"),
    handleUncoupleSong: (path: string) =>
      apply(uncoupleSong(path), "Failed to uncouple song"),
  };
}
