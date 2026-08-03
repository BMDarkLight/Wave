import { useEffect, useState } from "react";
import { BiArrowBack } from "react-icons/bi";
import {
  getAlbumTracks,
  getTrackFullCover,
  resolveCoverSrc,
} from "../utils/player";
import type { Track, PlaybackState } from "../utils/player";
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
  fallback,
  className,
  preferFull = false,
}: {
  track?: Track | null;
  fallback: string;
  className: string;
  /** When true, upgrade from thumb to full embedded cover. */
  preferFull?: boolean;
}) => {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);

    void (async () => {
      const thumb = track?.cover_art_data_url ?? null;
      if (thumb) {
        const resolved = await resolveCoverSrc(thumb);
        if (!cancelled && resolved) setSrc(resolved);
      }

      if (!preferFull || !track?.path) return;
      try {
        const full = await getTrackFullCover(track.path);
        if (!cancelled && full) setSrc(full);
      } catch {
        // Keep thumb / letter fallback.
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [track?.path, track?.cover_art_data_url, preferFull]);

  if (src) {
    return (
      <img
        className={className}
        src={src}
        alt={`${fallback} cover`}
        draggable={false}
      />
    );
  }
  return <div className={className}>{fallback}</div>;
};

interface AlbumPageProps {
  album: string;
  albumArtist: string | null;
  onBack: () => void;
  onPlayTrack: (path: string, tracks: Track[]) => void;
  onArtistClick: (artist: string) => void;
  playbackState: PlaybackState;
}

export default function AlbumPage({
  album,
  albumArtist,
  onBack,
  onPlayTrack,
  onArtistClick,
  playbackState,
}: AlbumPageProps) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    getAlbumTracks(album, albumArtist)
      .then(setTracks)
      .catch(() => setTracks([]))
      .finally(() => setLoading(false));
  }, [album, albumArtist]);

  const displayArtist = albumArtist || tracks[0]?.artist || "Unknown Artist";
  const year = tracks[0]?.year;
  const coverTrack = tracks[0];

  const isCurrentTrack = (track: Track) =>
    playbackState.current_path === track.path;

  const totalDuration = tracks.reduce(
    (sum, t) => sum + (t.duration_seconds || 0),
    0,
  );
  const totalMin = Math.floor(totalDuration / 60);

  if (loading) {
    return (
      <div className="main-content page-with-float-back">
        <button className="page-back-btn" onClick={onBack} type="button">
          <BiArrowBack />
        </button>
        <div className="empty-state">
          <div className="empty-icon">
            <span className="import-spinner" />
          </div>
          <h2>Loading album…</h2>
        </div>
      </div>
    );
  }

  return (
    <div className="main-content page-with-float-back">
      <button className="page-back-btn" onClick={onBack} type="button">
        <BiArrowBack />
      </button>
      <div className="album-hero">
        <Artwork
          track={coverTrack}
          fallback={album.slice(0, 2).toUpperCase()}
          className="album-hero-cover"
          preferFull
        />
        <div className="album-hero-info">
          <span className="album-hero-type">Album</span>
          <h1 className="album-hero-title">{album}</h1>
          <div className="album-hero-meta">
            <button
              className="album-hero-artist-btn"
              onClick={() => onArtistClick(displayArtist)}
              type="button"
            >
              {displayArtist}
            </button>
            {year && <span>· {year}</span>}
            <span>
              · {tracks.length} songs, about {totalMin} min
            </span>
          </div>
        </div>
      </div>

      <section className="playlist-container">
        {tracks.length === 0 ? (
          <div className="empty-state">
            <h2>No tracks found</h2>
          </div>
        ) : (
          <div
            className="track-list track-list-compact album-track-list"
            style={
              {
                "--track-grid": "52px minmax(120px, 1fr) 72px",
              } as React.CSSProperties
            }
          >
            <div className="track-list-header">
              <div className="track-col-index" title="Track number">
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
                const trackNo =
                  track.track_number != null && track.track_number > 0
                    ? String(track.track_number)
                    : "—";
                return (
                  <div
                    className={`track-item ${isCurrentTrack(track) ? "active" : ""}`}
                    onClick={() => onPlayTrack(track.path, tracks)}
                  >
                    <div className="track-col-index track-col-number">
                      {isCurrentTrack(track) && playbackState.is_playing ? (
                        <span className="mini-bars">
                          <i />
                          <i />
                          <i />
                        </span>
                      ) : (
                        trackNo
                      )}
                    </div>
                    <div className="track-title-cell">
                      <Artwork
                        track={track}
                        fallback={getTrackTitle(track).slice(0, 1).toUpperCase()}
                        className="track-thumb"
                      />
                      <div>
                        <div className="track-name">{getTrackTitle(track)}</div>
                        {track.artist && (
                          <div className="track-meta">{track.artist}</div>
                        )}
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
    </div>
  );
}
