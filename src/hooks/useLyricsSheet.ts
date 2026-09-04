import { useEffect, useState } from "react";
import { parseLyricsSheet, type LyricsSheet } from "../utils/player";

/**
 * Parse lyrics text into a structured sheet.
 *
 * Parsing runs in Rust so plain text, LRC, Enhanced LRC, and TTML all resolve
 * through one tested implementation, shared by the desktop panel and the
 * mobile now-playing view. A parse failure yields `null`, which callers render
 * as the raw text — lyrics that cannot be timed are still lyrics.
 */
export function useLyricsSheet(text?: string | null): LyricsSheet | null {
  const [sheet, setSheet] = useState<LyricsSheet | null>(null);

  useEffect(() => {
    if (!text) {
      setSheet(null);
      return;
    }
    let cancelled = false;
    parseLyricsSheet(text)
      .then((parsed) => {
        if (!cancelled) setSheet(parsed);
      })
      .catch(() => {
        if (!cancelled) setSheet(null);
      });
    return () => {
      cancelled = true;
    };
  }, [text]);

  return sheet;
}
