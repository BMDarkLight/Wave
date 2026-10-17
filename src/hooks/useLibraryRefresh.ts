/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  listenToLibraryRefreshFinished,
  requestLibraryRefresh,
} from "../utils/remote";

const COOLDOWN_SECONDS = 30;
/** Stop waiting for remote sources after this long; the local part is done. */
const REMOTE_WAIT_MS = 5 * 60 * 1000;

/**
 * Rescans local folders and syncs remote sources, at most once every 30
 * seconds. The backend keeps the clock, so every screen shares it.
 */
export function useLibraryRefresh({
  runLocalRescan,
  onFinished,
}: {
  runLocalRescan: () => Promise<void>;
  onFinished: () => void;
}) {
  const [refreshing, setRefreshing] = useState(false);
  const [cooldownSeconds, setCooldownSeconds] = useState(0);
  const latest = useRef({ runLocalRescan, onFinished });
  const busy = useRef(false);

  useEffect(() => {
    latest.current = { runLocalRescan, onFinished };
  }, [runLocalRescan, onFinished]);

  useEffect(() => {
    if (cooldownSeconds <= 0) return;
    const timer = window.setTimeout(
      () => setCooldownSeconds((seconds) => Math.max(0, seconds - 1)),
      1000,
    );
    return () => window.clearTimeout(timer);
  }, [cooldownSeconds]);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    let stopListening: (() => void) | null = null;
    try {
      // Listen first: with no remote work the backend finishes at once.
      let remoteDone!: () => void;
      const remoteFinished = new Promise<void>((resolve) => {
        remoteDone = resolve;
      });
      stopListening = await listenToLibraryRefreshFinished(() => remoteDone());
      const ticket = await requestLibraryRefresh();
      if (!ticket.started) {
        setCooldownSeconds(ticket.retry_after_seconds);
        return;
      }
      setRefreshing(true);
      const giveUp = new Promise<void>((resolve) =>
        window.setTimeout(resolve, REMOTE_WAIT_MS),
      );
      await Promise.all([
        latest.current.runLocalRescan().catch(() => {}),
        Promise.race([remoteFinished, giveUp]),
      ]);
      latest.current.onFinished();
      setCooldownSeconds(COOLDOWN_SECONDS);
    } catch {
      // A refresh that could not start leaves everything as it was.
    } finally {
      stopListening?.();
      setRefreshing(false);
      busy.current = false;
    }
  }, []);

  return { refresh, refreshing, cooldownSeconds };
}
