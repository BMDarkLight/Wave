/**
 * Lyrics parsing lives in Rust (`src-tauri/src/lyrics.rs`) so that plain text,
 * LRC, Enhanced LRC, and TTML share one tested implementation.
 *
 * What remains here is the one question the UI needs to answer synchronously:
 * does this text already carry timings? That gates the "fetch lyrics for this
 * track" effect, which must not await a round trip just to decide it has
 * nothing to do.
 */

/** LRC line tag `[mm:ss]`, or a TTML `begin` attribute. */
const TIMESTAMP_RE = /\[\d{1,3}:\d{2}(?:[.:]\d{1,3})?\]|\bbegin\s*=\s*["']/;

export const hasTimestamps = (raw?: string | null): boolean =>
  !!raw && TIMESTAMP_RE.test(raw);
