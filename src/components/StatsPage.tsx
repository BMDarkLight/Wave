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
import { BiBarChartAlt2, BiChevronLeft, BiPlay } from "react-icons/bi";
import {
  formatListenDuration,
  getListeningStats,
  type ListenRank,
  type ListeningStats,
  type Track,
} from "../utils/player";
import { hasListening } from "../utils/insights";
import InsightGrid from "./InsightGrid";

/** How many of each list the page shows. */
const RANKED = 20;
/** How often the page reads the totals again while it is open. */
const REFRESH_MS = 30_000;

const count = new Intl.NumberFormat();
const titleOf = (track: Track) => track.title || track.name || "Unknown";

/** Everything Wave knows about how you listen. */
export default function StatsPage({
  onPlayTrack,
  onOpenArtist,
  onBack,
}: {
  onPlayTrack: (path: string, tracks: Track[]) => void;
  onOpenArtist: (artist: string) => void;
  /** Back to Home. Only shown on the phone layout. */
  onBack: () => void;
}) {
  const [stats, setStats] = useState<ListeningStats | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      getListeningStats(RANKED)
        .then((next) => {
          if (!cancelled) setStats(next);
        })
        .catch(() => {})
        .finally(() => {
          if (!cancelled) setLoaded(true);
        });
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const any = hasListening(stats);

  return (
    <main className="main-content playlist-page stats-page">
      <div className="hero-copy">
        <div className="hero-top">
          <button
            className="mlib-icon-btn playlist-back-btn"
            onClick={onBack}
            type="button"
            title="Back"
            aria-label="Back to Home"
          >
            <BiChevronLeft />
          </button>
          <h1>Your listening</h1>
        </div>
        <p>Counted from what you play in Wave, on this device.</p>
      </div>

      <section className="stats-body">
        {!loaded ? null : !stats || !any ? (
          <div className="stats-empty">
            <BiBarChartAlt2 className="stats-empty-icon" aria-hidden />
            <h2>Nothing to show yet</h2>
            <p>
              Play some music and Wave will keep track of what you listen to
              most, and what that says about you.
            </p>
          </div>
        ) : (
          <>
            <div className="stats-totals">
              <Total
                value={formatListenDuration(stats.total_listen_seconds)}
                label="Listening time"
              />
              <Total value={count.format(stats.total_plays)} label="Plays" />
              <Total
                value={count.format(stats.tracks_played)}
                label="Songs played"
              />
              <Total value={count.format(stats.total_skips)} label="Skips" />
            </div>

            <InsightGrid stats={stats} />

            {stats.top_tracks.length > 0 && (
              <div className="stats-group">
                <h2>Top songs</h2>
                <ol className="stats-songs">
                  {stats.top_tracks.map((track, index) => (
                    <li key={track.path}>
                      <button
                        type="button"
                        className="stats-song"
                        onClick={() =>
                          onPlayTrack(track.path, stats.top_tracks)
                        }
                      >
                        <span className="stats-rank">{index + 1}</span>
                        <span className="stats-song-text">
                          <span className="stats-song-title">
                            {titleOf(track)}
                          </span>
                          {track.artist && (
                            <span className="stats-song-artist">
                              {track.artist}
                            </span>
                          )}
                        </span>
                        <BiPlay className="stats-song-play" aria-hidden />
                      </button>
                    </li>
                  ))}
                </ol>
              </div>
            )}

            <div className="stats-columns">
              <RankList
                title="Top artists"
                items={stats.top_artists}
                onOpen={onOpenArtist}
              />
              <RankList title="Top albums" items={stats.top_albums} />
              <RankList title="Top genres" items={stats.top_genres} />
            </div>
          </>
        )}
      </section>
    </main>
  );
}

function Total({ value, label }: { value: string; label: string }) {
  return (
    <div className="stats-total">
      <span className="stats-total-value">{value}</span>
      <span className="stats-total-label">{label}</span>
    </div>
  );
}

/** A ranked list with each row's share drawn as a bar behind it. */
function RankList({
  title,
  items,
  onOpen,
}: {
  title: string;
  items: ListenRank[];
  onOpen?: (name: string) => void;
}) {
  if (items.length === 0) return null;
  const top = Math.max(...items.map((item) => item.listen_seconds), 1);
  return (
    <div className="stats-group">
      <h2>{title}</h2>
      <ol className="stats-rank-list">
        {items.map((item, index) => {
          const body = (
            <>
              <span
                className="stats-bar"
                style={{ width: `${(item.listen_seconds / top) * 100}%` }}
                aria-hidden
              />
              <span className="stats-rank">{index + 1}</span>
              <span className="stats-rank-name">{item.name}</span>
              <span className="stats-rank-meta">
                {formatListenDuration(item.listen_seconds)}
              </span>
            </>
          );
          return (
            <li key={`${title}-${item.name}`}>
              {onOpen ? (
                <button
                  type="button"
                  className="stats-rank-row"
                  onClick={() => onOpen(item.name)}
                >
                  {body}
                </button>
              ) : (
                <div className="stats-rank-row">{body}</div>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
