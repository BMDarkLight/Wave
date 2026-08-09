import { useEffect, useState } from "react";
import { BiMusic } from "react-icons/bi";
import {
  getMostPlayed,
  getRecentlyPlayed,
  getTrackFullCover,
  resolveCoverSrc,
} from "../utils/player";
import type { PlaybackState, Track } from "../utils/player";
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

const Artwork = ({
  track,
  className,
}: {
  track: Track;
  className: string;
}) => {
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
  playbackState: PlaybackState;
}

export default function PlayedTracksPage({
  mode,
  onPlayTrack,
  playbackState,
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

  if (loading) {
    return (
      <main className="main-content">
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
    <main className="main-content">
      <div className="hero-copy">
        <div className="hero-top">
          <h1>{title}</h1>
        </div>
        <p>{subtitle}</p>
      </div>

      <section className="playlist-container">
        {tracks.length === 0 ? (
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
                "--track-grid": "36px minmax(0, 1fr) 78px",
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
              count={tracks.length}
              estimateSize={58}
              className="track-list-virtual"
            >
              {(i) => {
                const track = tracks[i];
                if (!track) return null;
                const isCurrent = playbackState.current_path === track.path;
                return (
                  <div
                    key={track.path}
                    className={`track-item${isCurrent ? " active" : ""}`}
                    onClick={() => onPlayTrack(track.path, tracks)}
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
                      <Artwork track={track} className="track-thumb" />
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
