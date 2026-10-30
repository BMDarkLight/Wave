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
  listWavePeers,
  searchWavePeers,
  onWavePeerAnswered,
  onWavePeerDeclined,
  onWavePeerFound,
  onWavePeerRequest,
  onWavePeersChanged,
  type PeerEvent,
  type WavePeer,
} from "../utils/peers";

/** One thing the window should ask about another Wave, oldest first. */
export type PeerPrompt =
  | { kind: "found"; peer: PeerEvent }
  | { kind: "request"; peer: PeerEvent }
  | { kind: "share-back"; peer: PeerEvent }
  | { kind: "declined"; peer: PeerEvent };

/** How often the list is read again while nothing announces a change, for
 *  the online dots and "playing from you". */
const REFRESH_MS = 10_000;
/** How long a search runs before "Searching…" goes, with the list read
 *  again along the way. Discovery searches at 0, 2 and 6 seconds. */
const SEARCH_MS = 8_000;
const SEARCH_REFRESH_MS = 2_000;

export function useWavePeers(enabled: boolean) {
  const [peers, setPeers] = useState<WavePeer[]>([]);
  const [prompts, setPrompts] = useState<PeerPrompt[]>([]);
  const [searching, setSearching] = useState(false);
  const searchTimers = useRef<number[]>([]);

  const refresh = useCallback(() => {
    listWavePeers()
      .then(setPeers)
      .catch(() => {});
  }, []);

  /** Shows a search for as long as discovery's own searches take, reading
   *  the list as answers come in. */
  const showSearch = useCallback(() => {
    searchTimers.current.forEach((timer) => window.clearTimeout(timer));
    setSearching(true);
    searchTimers.current = [
      ...Array.from({ length: SEARCH_MS / SEARCH_REFRESH_MS }, (_, i) =>
        window.setTimeout(refresh, (i + 1) * SEARCH_REFRESH_MS),
      ),
      window.setTimeout(() => setSearching(false), SEARCH_MS),
    ];
  }, [refresh]);

  const search = useCallback(() => {
    searchWavePeers().catch(() => {});
    showSearch();
  }, [showSearch]);

  useEffect(
    () => () => searchTimers.current.forEach((t) => window.clearTimeout(t)),
    [],
  );

  useEffect(() => {
    if (!enabled) return;
    // Discovery searches as soon as Outside Sourcing is on.
    showSearch();
    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    const stops: (() => void)[] = [];
    let cancelled = false;
    const keep = (stop: () => void) => {
      if (cancelled) stop();
      else stops.push(stop);
    };
    const add = (prompt: PeerPrompt) =>
      setPrompts((queue) =>
        // One prompt per device at a time; the newest says the most.
        [...queue.filter((p) => p.peer.id !== prompt.peer.id), prompt],
      );
    onWavePeerFound((peer) => add({ kind: "found", peer }))
      .then(keep)
      .catch(() => {});
    onWavePeerRequest((peer) => {
      add({ kind: "request", peer });
      refresh();
    })
      .then(keep)
      .catch(() => {});
    onWavePeerAnswered((peer) => {
      add({ kind: "share-back", peer });
      refresh();
    })
      .then(keep)
      .catch(() => {});
    onWavePeerDeclined((peer) => {
      add({ kind: "declined", peer });
      refresh();
    })
      .then(keep)
      .catch(() => {});
    onWavePeersChanged(refresh)
      .then(keep)
      .catch(() => {});
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      stops.forEach((stop) => stop());
    };
  }, [enabled, refresh, showSearch]);

  const settle = useCallback((id: string) => {
    setPrompts((queue) => queue.filter((p) => p.peer.id !== id));
  }, []);

  return {
    peers,
    prompts,
    current: prompts[0] ?? null,
    settle,
    refresh,
    searching: enabled && searching,
    search,
  };
}

export type WavePeers = ReturnType<typeof useWavePeers>;
