/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import type { ListeningStats } from "./player";

/** One thing worth saying about how someone listens. */
export interface Insight {
  id: string;
  /** The headline number or name. */
  value: string;
  text: string;
}

const count = new Intl.NumberFormat();
const percent = (part: number, whole: number) =>
  whole > 0 ? Math.round((part / whole) * 100) : 0;

/** Whether there is anything to show at all. */
export const hasListening = (stats: ListeningStats | null): boolean =>
  !!stats && (stats.tracks_played > 0 || stats.total_listen_seconds > 0);

/**
 * What the totals say, most telling first. Each one needs enough listening
 * behind it to mean something, so a new library gets few or none.
 */
export function listeningInsights(stats: ListeningStats): Insight[] {
  const out: Insight[] = [];

  const artist = stats.top_artists[0];
  if (artist && stats.total_listen_seconds >= 30 * 60) {
    const share = percent(artist.listen_seconds, stats.total_listen_seconds);
    if (share >= 10) {
      out.push({
        id: "top-artist",
        value: `${share}%`,
        text: `of your listening is ${artist.name}`,
      });
    }
  }

  const started = stats.total_plays + stats.total_skips;
  if (started >= 10) {
    out.push({
      id: "finish-rate",
      value: `${percent(stats.total_plays, started)}%`,
      text: "of the songs you start, you hear to the end",
    });
  }

  if (stats.played_this_week > 0) {
    out.push({
      id: "this-week",
      value: count.format(stats.played_this_week),
      text:
        stats.played_this_week === 1
          ? "song played this week"
          : "different songs played this week",
    });
  }

  if (stats.library_tracks >= 20) {
    const explored = percent(stats.tracks_played, stats.library_tracks);
    out.push({
      id: "explored",
      value: `${explored}%`,
      text:
        explored < 25
          ? `of your ${count.format(stats.library_tracks)} songs played so far; plenty left to find`
          : `of your ${count.format(stats.library_tracks)} songs played so far`,
    });
  }

  const genre = stats.top_genres[0];
  if (genre) {
    out.push({
      id: "top-genre",
      value: genre.name,
      text: "is the genre you play most",
    });
  }

  if (stats.total_plays >= 5) {
    const average = stats.total_listen_seconds / stats.total_plays;
    out.push({
      id: "per-play",
      value: `${Math.floor(average / 60)}:${String(Math.round(average % 60)).padStart(2, "0")}`,
      text: "listened on average each time you play a song",
    });
  }

  return out;
}
