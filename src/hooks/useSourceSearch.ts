import { useCallback, useEffect, useRef, useState } from "react";
import {
  downloadSourceTrack,
  searchSources,
  streamSourceTrack,
  type ProviderResults,
  type SourceTrack,
  type Track,
} from "../utils/player";

/** Stable key for one remote result, since ids are only unique per provider. */
export const sourceTrackKey = (track: SourceTrack) =>
  `${track.provider}:${track.id}`;

/**
 * Tier 3 of the search ladder: remote sources.
 *
 * Deliberately manual. The scope and library tiers run on every keystroke
 * because they are local; this one costs an HTTP request per provider, so it
 * fires only when the user presses the button — and resets the moment the
 * query changes, so stale remote results can never sit under a new search.
 */
export function useSourceSearch(query: string) {
  const [results, setResults] = useState<ProviderResults[]>([]);
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Keys of tracks with a stream or download in flight. */
  const [busy, setBusy] = useState<Record<string, "stream" | "download">>({});

  const reqId = useRef(0);

  const reset = useCallback(() => {
    // Invalidate any in-flight request so its response is dropped.
    reqId.current += 1;
    setResults([]);
    setSearched(false);
    setLoading(false);
    setError(null);
  }, []);

  // A new query invalidates everything the previous one found.
  useEffect(() => {
    reset();
  }, [query, reset]);

  const search = useCallback(async () => {
    const q = query.trim();
    if (!q) return;
    const id = ++reqId.current;
    setLoading(true);
    setError(null);
    try {
      const sections = await searchSources(q, 20);
      if (reqId.current !== id) return;
      setResults(sections);
      setSearched(true);
    } catch (e) {
      if (reqId.current !== id) return;
      setResults([]);
      setSearched(true);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (reqId.current === id) setLoading(false);
    }
  }, [query]);

  const markBusy = (key: string, kind: "stream" | "download" | null) => {
    setBusy((prev) => {
      if (!kind) {
        const { [key]: _removed, ...rest } = prev;
        return rest;
      }
      return { ...prev, [key]: kind };
    });
  };

  /** Fetch a remote track and hand back a playable library row. */
  const stream = useCallback(async (track: SourceTrack): Promise<Track> => {
    const key = sourceTrackKey(track);
    markBusy(key, "stream");
    try {
      return await streamSourceTrack(track);
    } finally {
      markBusy(key, null);
    }
  }, []);

  /** Keep a remote track, marking the row as owned once it lands. */
  const download = useCallback(async (track: SourceTrack): Promise<Track> => {
    const key = sourceTrackKey(track);
    markBusy(key, "download");
    try {
      const saved = await downloadSourceTrack(track);
      // Reflect ownership immediately so the row stops offering a download.
      setResults((prev) =>
        prev.map((section) => ({
          ...section,
          tracks: section.tracks.map((t) =>
            sourceTrackKey(t) === key
              ? { ...t, already_in_library: saved.path }
              : t,
          ),
        })),
      );
      return saved;
    } finally {
      markBusy(key, null);
    }
  }, []);

  const totalHits = results.reduce((sum, r) => sum + r.tracks.length, 0);

  return {
    sourceResults: results,
    sourceLoading: loading,
    sourceSearched: searched,
    sourceError: error,
    sourceBusy: busy,
    sourceTotalHits: totalHits,
    searchSourcesNow: search,
    resetSourceSearch: reset,
    streamSourceHit: stream,
    downloadSourceHit: download,
  };
}
