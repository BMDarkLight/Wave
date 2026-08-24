export type LyricLine = { time: number; text: string };

const LRC_TAG_RE = /\[(\d{1,2}):(\d{2})(?:\.(\d{1,3}))?\]/g;

/** Parses LRC-style "[mm:ss.xx] text" lyrics into timestamped lines. Returns
 * null if the text doesn't look like it has real timestamps (plain lyrics),
 * so the caller can fall back to rendering the raw text. */
export const parseTimedLyrics = (raw?: string | null): LyricLine[] | null => {
  if (!raw) return null;
  const lines = raw.split(/\r?\n/);
  const result: LyricLine[] = [];
  let matchedLines = 0;
  let nonEmptyLines = 0;

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    nonEmptyLines++;
    const tags = [...trimmed.matchAll(LRC_TAG_RE)];
    if (tags.length === 0) continue;
    matchedLines++;
    const text = trimmed.replace(LRC_TAG_RE, "").trim();
    for (const tag of tags) {
      const minutes = parseInt(tag[1], 10);
      const seconds = parseInt(tag[2], 10);
      const fraction = tag[3] ? parseFloat(`0.${tag[3]}`) : 0;
      result.push({ time: minutes * 60 + seconds + fraction, text });
    }
  }

  if (nonEmptyLines === 0 || matchedLines < nonEmptyLines * 0.4) return null;
  result.sort((a, b) => a.time - b.time);
  return result;
};
