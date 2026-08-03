import { useEffect, useState } from "react";
import {
  BiArrowBack,
  BiChevronDown,
  BiChevronRight,
} from "react-icons/bi";
import {
  getArtistTracks,
  getArtistAlbums,
  getTrackFullCover,
  resolveCoverSrc,
} from "../utils/player";
import type { Track, PlaybackState, AlbumSummary } from "../utils/player";
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
}: {
  track?: Track | null;
  fallback: string;
  className: string;
}) => {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void resolveCoverSrc(track?.cover_art_data_url).then((resolved) => {
      if (!cancelled) setSrc(resolved);
    });
    return () => {
      cancelled = true;
    };
  }, [track?.cover_art_data_url]);
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

const AlbumArt = ({
  album,
  className,
}: {
  album: AlbumSummary;
  className: string;
}) => {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setSrc(null);

    void (async () => {
      if (album.cover_art_data_url) {
        const resolved = await resolveCoverSrc(album.cover_art_data_url);
        if (!cancelled && resolved) setSrc(resolved);
      }
      const path = album.cover_track_path;
      if (!path) return;
      try {
        const full = await getTrackFullCover(path);
        if (!cancelled && full) setSrc(full);
      } catch {
        // Keep thumb / letter fallback.
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [album.cover_art_data_url, album.cover_track_path]);

  if (src) {
    return (
      <img
        className={className}
        src={src}
        alt={`${album.name} cover`}
        draggable={false}
      />
    );
  }
  return <div className={className}>{album.name.slice(0, 2).toUpperCase()}</div>;
};

interface ArtistPageProps {
  artist: string;
  onBack: () => void;
  onPlayTrack: (path: string, tracks: Track[]) => void;
  onAlbumClick: (album: string, albumArtist: string | null) => void;
  playbackState: PlaybackState;
}

export default function ArtistPage({
  artist,
  onBack,
  onPlayTrack,
  onAlbumClick,
  playbackState,
}: ArtistPageProps) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [songsOpen, setSongsOpen] = useState(false);

  useEffect(() => {
    setLoading(true);
    Promise.all([getArtistTracks(artist), getArtistAlbums(artist)])
      .then(([t, a]) => {
        setTracks(t);
        setAlbums(a);
      })
      .catch(() => {
        setTracks([]);
        setAlbums([]);
      })
      .finally(() => setLoading(false));
  }, [artist]);

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
          <h2>Loading artist…</h2>
        </div>
      </div>
    );
  }

  return (
    <div className="main-content page-with-float-back">
      <button className="page-back-btn" onClick={onBack} type="button">
        <BiArrowBack />
      </button>

      {/* Artist hero */}
      <div className="artist-hero">
        <h1 className="artist-hero-title">{artist}</h1>
        <div className="artist-hero-meta">
          {albums.length > 0 && (
            <span>
              {albums.length} album{albums.length !== 1 ? "s" : ""}
            </span>
          )}
          {tracks.length > 0 && (
            <span>
              · {tracks.length} song{tracks.length !== 1 ? "s" : ""}, about{" "}
              {totalMin} min
            </span>
          )}
        </div>
      </div>

      {/* Albums collection */}
      {albums.length > 0 && (
        <section className="artist-section">
          <h2 className="artist-section-title">Albums</h2>
          <div className="artist-album-grid">
            {albums.map((album) => (
              <button
                key={`${album.name}-${album.album_artist}`}
                className="artist-album-card"
                onClick={() => onAlbumClick(album.name, album.album_artist)}
                type="button"
              >
                <AlbumArt album={album} className="artist-album-card-cover" />
                <div className="artist-album-card-name">{album.name}</div>
                <div className="artist-album-card-meta">
                  {album.year && <span>{album.year} · </span>}
                  {album.track_count} song{album.track_count !== 1 ? "s" : ""}
                </div>
              </button>
            ))}
          </div>
        </section>
      )}

      {/* Discography */}
      {albums.length > 0 && (
        <section className="artist-section">
          <h2 className="artist-section-title">Discography</h2>
          <div className="artist-discography-list">
            {albums.map((album) => (
              <button
                key={`disc-${album.name}-${album.album_artist}`}
                className="artist-discography-item"
                onClick={() => onAlbumClick(album.name, album.album_artist)}
                type="button"
              >
                <AlbumArt
                  album={album}
                  className="artist-discography-item-cover"
                />
                <div className="artist-discography-item-info">
                  <div className="artist-discography-item-name">
                    {album.name}
                  </div>
                  <div className="artist-discography-item-meta">
                    {album.year && <span>{album.year} · </span>}
                    {album.track_count} song
                    {album.track_count !== 1 ? "s" : ""}
                  </div>
                </div>
              </button>
            ))}
          </div>
        </section>
      )}

      {/* Songs (collapsible) */}
      {tracks.length > 0 && (
        <section className="artist-section">
          <button
            className="artist-section-title artist-collapsible-header"
            onClick={() => setSongsOpen((v) => !v)}
            type="button"
          >
            {songsOpen ? <BiChevronDown /> : <BiChevronRight />}
            Songs
          </button>
          {songsOpen && (
            <div
              className="track-list track-list-compact"
              style={
                {
                  "--track-grid": "48px minmax(80px, 1fr) 60px",
                } as React.CSSProperties
              }
            >
              <div className="track-list-header">
                <div className="track-col-index">#</div>
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
                  return (
                    <div
                      className={`track-item ${isCurrentTrack(track) ? "active" : ""}`}
                      onClick={() => onPlayTrack(track.path, tracks)}
                    >
                      <div className="track-col-index">
                        {isCurrentTrack(track) && playbackState.is_playing ? (
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
                        <Artwork
                          track={track}
                          fallback={getTrackTitle(track).slice(0, 1).toUpperCase()}
                          className="track-thumb"
                        />
                        <div>
                          <div className="track-name">{getTrackTitle(track)}</div>
                          <div className="track-meta">{track.album}</div>
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
      )}
    </div>
  );
}
