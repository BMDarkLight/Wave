import type { ReactNode } from "react";
import type { SearchHit, Track } from "./player";

export const MATCH_FIELD_LABEL: Record<string, string> = {
  title: "Title",
  artist: "Artist",
  album: "Album",
  name: "File",
  lyrics: "Lyrics",
};

export type MainSearchScope =
  | { kind: "library" }
  | { kind: "playlist"; label: string; paths: Set<string> }
  | { kind: "album"; name: string; albumArtist: string | null }
  | { kind: "artist"; name: string };

export function trackInAlbum(
  track: Track,
  album: string,
  albumArtist: string | null,
): boolean {
  if (track.album !== album) return false;
  if (!albumArtist) return true;
  const aa = track.album_artist || track.artist;
  return aa === albumArtist;
}

export function trackByArtist(track: Track, artist: string): boolean {
  return track.artist === artist || track.album_artist === artist;
}

export function hitMatchesSearchScope(
  hit: SearchHit,
  scope: MainSearchScope,
): boolean {
  const track = hit.track;
  switch (scope.kind) {
    case "library":
      return true;
    case "playlist":
      return scope.paths.has(track.path);
    case "album":
      return trackInAlbum(track, scope.name, scope.albumArtist);
    case "artist":
      return trackByArtist(track, scope.name);
  }
}

export function mainSearchScopeLabel(scope: MainSearchScope): string {
  switch (scope.kind) {
    case "library":
      return "library";
    case "playlist":
      return scope.label;
    case "album":
      return scope.name;
    case "artist":
      return scope.name;
  }
}

export function highlightMatch(text: string, query: string): ReactNode {
  const q = query.trim();
  if (!q || !text) return text;
  const tokens = q
    .split(/\s+/)
    .filter(Boolean)
    .sort((a, b) => b.length - a.length);
  if (!tokens.length) return text;

  const lower = text.toLowerCase();
  const ranges: Array<[number, number]> = [];
  for (const token of tokens) {
    const needle = token.toLowerCase();
    let from = 0;
    while (from < lower.length) {
      const idx = lower.indexOf(needle, from);
      if (idx < 0) break;
      ranges.push([idx, idx + needle.length]);
      from = idx + needle.length;
    }
  }
  if (!ranges.length) return text;
  ranges.sort((a, b) => a[0] - b[0]);
  const merged: Array<[number, number]> = [];
  for (const range of ranges) {
    const last = merged[merged.length - 1];
    if (last && range[0] <= last[1]) {
      last[1] = Math.max(last[1], range[1]);
    } else {
      merged.push([...range] as [number, number]);
    }
  }

  const parts: ReactNode[] = [];
  let cursor = 0;
  merged.forEach(([start, end], i) => {
    if (cursor < start) parts.push(text.slice(cursor, start));
    parts.push(
      <mark key={`${start}-${i}`} className="search-hit-mark">
        {text.slice(start, end)}
      </mark>,
    );
    cursor = end;
  });
  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts;
}
