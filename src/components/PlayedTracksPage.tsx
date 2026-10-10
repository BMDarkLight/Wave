/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useMemo, useState } from "react";
import { BiChevronLeft, BiMusic, BiSearch } from "react-icons/bi";
import {
  getMostPlayed,
  getRecentlyPlayed,
  getTrackFullCover,
  resolveCoverSrc,
} from "../utils/player";
import type { PlaybackState, Track } from "../utils/player";
import { matchesQuery } from "../utils/track";
import type { ContextMenuAnchor } from "./ContextMenu";
import MobileFilterField from "./MobileFilterField";
import TrackMenuButton from "./TrackMenuButton";
import VirtualizedList from "./VirtualizedList";

const formatTime = (seconds?: number | null) => {
  if (!seconds || !Number.isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60)
    .toString()
    .padStart(2, "0");
  return `${m}:${s}`;
};

const getTrackTitle = (track?: Track | null) => {
  if (track?.title) return track.title;
  if (track?.name) return track.name;
  return "Unknown";
};

const Artwork = ({ track, className }: { track: Track; className: string }) => {
  const [src, setSrc] = useState<string | null>(null);
  const fallback = getTrackTitle(track).slice(0, 1).toUpperCase();

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    void (async () => {
      const thumb = track.cover_art_data_url;
      if (thumb) {
        const resolved = await resolveCoverSrc(thumb);
        if (!cancelled && resolved) setSrc(resolved);
      }
      if (!track.path) return;
      try {
        const full = await getTrackFullCover(track.path);
        if (!cancelled && full) setSrc(full);
      } catch {
        /* keep thumb */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [track.path, track.cover_art_data_url]);

  if (src) {
    return (
      <img
        className={className}
        src={src}
        alt={`${getTrackTitle(track)} cover`}
        draggable={false}
      />
    );
  }
  return <div className={className}>{fallback}</div>;
};

export type PlayedTracksMode = "recently_played" | "most_played";

interface PlayedTracksPageProps {
  mode: PlayedTracksMode;
  onPlayTrack: (path: string, tracks: Track[]) => void;
  onOpenTrackMenu: (track: Track, anchor: ContextMenuAnchor) => void;
  onCloseTrackMenu: () => void;
  menuTrackPath: string | null;
  playbackState: PlaybackState;
  /** Back to the Library tab. Only shown on the phone layout. */
  onBack: () => void;
}

export default function PlayedTracksPage({
  mode,
  onPlayTrack,
  onOpenTrackMenu,
  onCloseTrackMenu,
  menuTrackPath,
  playbackState,
  onBack,
}: PlayedTracksPageProps) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(true);

  const title = mode === "recently_played" ? "Recently Played" : "Most Played";
  const subtitle =
    mode === "recently_played"
      ? "Your latest listens"
      : "Your top songs by plays";

  useEffect(() => {
    let cancelled = false;
    let first = true;
    const load = () => {
      if (first) setLoading(true);
      const req =
        mode === "recently_played"
          ? getRecentlyPlayed(100)
          : getMostPlayed(100);
      void req
        .then((list) => {
          if (!cancelled) setTracks(list);
        })
        .catch(() => {
          if (!cancelled) setTracks([]);
        })
        .finally(() => {
          if (!cancelled) {
            setLoading(false);
            first = false;
          }
        });
    };
    load();
    // Refresh while the page is open so new listens show up without navigating away.
    const id = window.setInterval(load, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [mode]);

  // The phone layout's search narrows the list in place. Each shown row
  // keeps its rank in the full list.
  const [filtering, setFiltering] = useState(false);
  const [filter, setFilter] = useState("");
  const shownRows = useMemo(() => {
    const rows = tracks.map((track, index) => ({ track, index }));
    if (!filter.trim()) return rows;
    return rows.filter(({ track }) =>
      matchesQuery(filter, getTrackTitle(track), track.artist, track.album),
    );
  }, [tracks, filter]);
  const closeFilter = () => {
    setFilter("");
    setFiltering(false);
  };

  const pageClass = `main-content playlist-page${
    mode === "most_played" ? " playlist-page-ranked" : ""
  }`;

  if (loading) {
    return (
      <main className={pageClass}>
        <div className="empty-state">
          <div className="empty-icon">
            <span className="import-spinner" />
          </div>
          <h2>Loading {title.toLowerCase()}…</h2>
        </div>
      </main>
    );
  }

  return (
    <main className={pageClass}>
      <div className="hero-copy">
        <div className="hero-top">
          <button
            className="mlib-icon-btn playlist-back-btn"
            onClick={onBack}
            type="button"
            title="Back"
            aria-label="Back to library"
          >
            <BiChevronLeft />
          </button>
          <h1>{title}</h1>
          {tracks.length > 0 && (
            <div className="hero-actions">
              <button
                className={`btn-secondary playlist-filter-btn${filtering ? " active" : ""}`}
                onClick={() => (filtering ? closeFilter() : setFiltering(true))}
                type="button"
                title={`Search ${title.toLowerCase()}`}
                aria-label={`Search ${title.toLowerCase()}`}
                aria-expanded={filtering}
              >
                <BiSearch />
              </button>
            </div>
          )}
        </div>
        <p>
          {filter.trim()
            ? `${shownRows.length} of ${tracks.length} songs`
            : subtitle}
        </p>
      </div>

      <MobileFilterField
        open={filtering}
        label={`Search ${title.toLowerCase()}`}
        placeholder="Songs, artists or albums"
        query={filter}
        onQueryChange={setFilter}
        onClose={closeFilter}
      />

      <section className="playlist-container">
        {tracks.length > 0 && shownRows.length === 0 ? (
          <p className="mcol-status">Nothing matches “{filter.trim()}”</p>
        ) : tracks.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">
              <BiMusic />
            </div>
            <h2>No listening history yet</h2>
            <p>Play some music and Wave will rank your favorites here.</p>
          </div>
        ) : (
          <div
            className="track-list track-list-compact"
            style={
              {
                "--track-grid": "36px minmax(0, 1fr) 78px 40px",
              } as React.CSSProperties
            }
          >
            <div className="track-list-header">
              <div className="track-col-index" title="Rank">
                #
              </div>
              <div className="track-title-cell">Title</div>
              <div className="track-duration track-duration-header">
                Duration
              </div>
            </div>
            <VirtualizedList
              count={shownRows.length}
              estimateSize={58}
              className="track-list-virtual"
            >
              {(row) => {
                const shownRow = shownRows[row];
                if (!shownRow) return null;
                const { track, index: i } = shownRow;
                const isCurrent = playbackState.current_path === track.path;
                return (
                  <div
                    key={track.path}
                    className={`track-item${isCurrent ? " active" : ""}`}
                    onClick={() => onPlayTrack(track.path, tracks)}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      onOpenTrackMenu(track, {
                        top: event.clientY,
                        left: event.clientX,
                        flipAbove: event.clientY,
                      });
                    }}
                  >
                    <div className="track-col-index track-col-number">
                      {isCurrent && playbackState.is_playing ? (
                        <span className="mini-bars">
                          <i />
                          <i />
                          <i />
                        </span>
                      ) : (
                        i + 1
                      )}
                    </div>
                    <div className="track-title-cell">
                      <span className="track-thumb-wrap">
                        <Artwork track={track} className="track-thumb" />
                        {isCurrent && playbackState.is_playing && (
                          <span className="track-thumb-playing" aria-hidden>
                            <span className="mini-bars">
                              <i />
                              <i />
                              <i />
                            </span>
                          </span>
                        )}
                      </span>
                      <div>
                        <div className="track-name">{getTrackTitle(track)}</div>
                        <div className="track-meta">
                          {track.artist || "Unknown"}
                          {track.album ? ` · ${track.album}` : ""}
                        </div>
                      </div>
                    </div>
                    <div className="track-duration">
                      {formatTime(track.duration_seconds)}
                    </div>
                    <TrackMenuButton
                      track={track}
                      isOpen={menuTrackPath === track.path}
                      onOpen={onOpenTrackMenu}
                      onClose={onCloseTrackMenu}
                    />
                  </div>
                );
              }}
            </VirtualizedList>
          </div>
        )}
      </section>
    </main>
  );
}
