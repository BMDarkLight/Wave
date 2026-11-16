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
import { parseLyricsSheet, type LyricsSheet } from "../utils/player";

/**
 * Parse lyrics text into a structured sheet.
 *
 * Parsing runs in Rust so plain text, LRC, Enhanced LRC, and TTML all resolve
 * through one tested implementation, shared by the desktop panel and the
 * mobile now-playing view. A parse failure yields `null`, which callers render
 * as the raw text — lyrics that cannot be timed are still lyrics.
 *
 * A request id guards against out-of-order resolves: if the text changes
 * while a parse is in flight, the stale result is ignored instead of
 * overwriting the newer sheet.
 */
export function useLyricsSheet(text?: string | null): LyricsSheet | null {
  const [sheet, setSheet] = useState<LyricsSheet | null>(null);
  const requestIdRef = useRef(0);

  useEffect(() => {
    if (!text) {
      setSheet(null);
      return;
    }
    // Drop the previous sheet immediately so a track change never keeps
    // showing the previous song's parsed lyrics while the new one parses.
    setSheet(null);
    const requestId = ++requestIdRef.current;
    parseLyricsSheet(text)
      .then((parsed) => {
        if (requestIdRef.current !== requestId) return;
        setSheet(parsed);
      })
      .catch(() => {
        if (requestIdRef.current !== requestId) return;
        setSheet(null);
      });
    return () => {
      // Invalidate any in-flight parse for this text so a late resolve
      // cannot overwrite a newer sheet.
      if (requestIdRef.current === requestId) {
        requestIdRef.current += 1;
      }
    };
  }, [text]);

  return sheet;
}
