import { useEffect, useMemo, useState } from "react";
import { BiPlay, BiRefresh, BiMusic } from "react-icons/bi";
import {
  getPlaylistTracksById,
  getTrackFullCover,
  listAlbums,
  resolveCoverSrc,
} from "../utils/player";
import type { AlbumSummary, Track } from "../utils/player";

const getTrackTitle = (track?: Track | null) => {
  if (track?.title) return track.title;
  if (track?.name) return track.name;
  return "Unknown";
};

const UNKNOWN_RE = /^(unknown(\s+artist)?|various(\s+artists)?|untitled|unknown album)?$/i;

function hasRealValue(value?: string | null): boolean {
  const trimmed = value?.trim();
  if (!trimmed) return false;
  return !UNKNOWN_RE.test(trimmed);
}

/** Higher = richer tags / art. Used to bias Home suggestions. */
function trackMetadataScore(track: Track): number {
  let score = 0;
  const title = track.title?.trim();
  if (title && title !== track.name) score += 3;
  else if (hasRealValue(title) || hasRealValue(track.name)) score += 1;
  if (hasRealValue(track.artist)) score += 3;
  if (hasRealValue(track.album)) score += 2;
  if (track.cover_art_data_url || track.album_art_id) score += 4;
  return score;
}

function albumMetadataScore(album: AlbumSummary): number {
  let score = 0;
  if (hasRealValue(album.name) && album.name !== "Unknown Album") score += 2;
  if (hasRealValue(album.album_artist) || hasRealValue(album.artist)) score += 3;
  if (album.cover_art_data_url || album.cover_track_path) score += 4;
  return score;
}

function shufflePick<T>(items: T[], count: number): T[] {
  if (items.length === 0) return [];
  const copy = [...items];
  for (let i = copy.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [copy[i], copy[j]] = [copy[j], copy[i]];
  }
  return copy.slice(0, Math.min(count, copy.length));
}

/**
 * Prefer items with richer metadata while keeping picks random within
 * quality tiers so Refresh still feels fresh.
 */
function shufflePickPreferring<T>(
  items: T[],
  count: number,
  scoreOf: (item: T) => number,
  minPreferredScore: number,
): T[] {
  if (items.length === 0 || count <= 0) return [];
  const rich = items.filter((item) => scoreOf(item) >= minPreferredScore);
  const rest = items.filter((item) => scoreOf(item) < minPreferredScore);
  const preferred = shufflePick(rich, count);
  if (preferred.length >= count) return preferred;
  return [...preferred, ...shufflePick(rest, count - preferred.length)];
}

/** Album card art: show thumb immediately, then upgrade to full embedded cover. */
const AlbumCover = ({
  album,
  className,
}: {
  album: AlbumSummary;
  className: string;
}) => {
  const [src, setSrc] = useState<string | null>(null);
  const fallback = album.name.slice(0, 1).toUpperCase();

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
  return <div className={className}>{fallback}</div>;
};

const TrackCover = ({
  track,
  className,
  preferFull = false,
}: {
  track: Track;
  className: string;
  preferFull?: boolean;
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
      if (!preferFull || !track.path) return;
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
  }, [track.path, track.cover_art_data_url, preferFull]);

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

type HomePageProps = {
  libraryPlaylistId: string | null;
  onPlayTrack: (path: string, queue: Track[]) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onOpenArtist: (artist: string) => void;
  onOpenLibrary: () => void;
};

export default function HomePage({
  libraryPlaylistId,
  onPlayTrack,
  onOpenAlbum,
  onOpenArtist,
  onOpenLibrary,
}: HomePageProps) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [seed, setSeed] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const [albumList, libraryTracks] = await Promise.all([
          listAlbums().catch(() => [] as AlbumSummary[]),
          libraryPlaylistId
            ? getPlaylistTracksById(libraryPlaylistId).catch(() => [] as Track[])
            : Promise.resolve([] as Track[]),
        ]);
        if (cancelled) return;
        setAlbums(albumList);
        setTracks(libraryTracks);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [libraryPlaylistId]);

  const suggestions = useMemo(() => {
    void seed;
    // Prefer title+artist+album+cover (score ~12); fall back only if needed.
    return shufflePickPreferring(tracks, 18, trackMetadataScore, 10);
  }, [tracks, seed]);

  const featured = suggestions[0] ?? null;
  const mixRow = suggestions.slice(1, 7);
  const moreRow = suggestions.slice(7, 15);

  const albumPicks = useMemo(() => {
    void seed;
    const candidates = albums.filter(
      (a) => hasRealValue(a.name) && a.name !== "Unknown Album",
    );
    return shufflePickPreferring(candidates, 10, albumMetadataScore, 7);
  }, [albums, seed]);

  const greeting = useMemo(() => {
    const hour = new Date().getHours();
    if (hour < 5) return "Up late";
    if (hour < 12) return "Good morning";
    if (hour < 18) return "Good afternoon";
    return "Good evening";
  }, []);

  const playFeatured = () => {
    if (!featured) return;
    const rest = suggestions.filter((t) => t.path !== featured.path);
    onPlayTrack(featured.path, [featured, ...rest]);
  };

  if (loading) {
    return (
      <main className="main-content home-page">
        <div className="home-loading">Finding something to play…</div>
      </main>
    );
  }

  if (tracks.length === 0) {
    return (
      <main className="main-content home-page">
        <div className="home-empty">
          <BiMusic />
          <h1>Your home is empty</h1>
          <p>Scan a music folder into Library to unlock suggestions and covers.</p>
          <button className="btn-primary" type="button" onClick={onOpenLibrary}>
            Go to Library
          </button>
        </div>
      </main>
    );
  }

  return (
    <main className="main-content home-page">
      <header className="home-header">
        <div>
          <p className="home-eyebrow">Wave</p>
          <h1>{greeting}</h1>
          <p className="home-sub">
            Picked from {tracks.length.toLocaleString()} tracks in your library
          </p>
        </div>
        <button
          className="home-refresh-btn"
          type="button"
          onClick={() => setSeed((n) => n + 1)}
          title="Shuffle suggestions"
          aria-label="Shuffle suggestions"
        >
          <BiRefresh />
          Refresh
        </button>
      </header>

      {featured && (
        <section className="home-featured" aria-label="Featured track">
          <div className="home-featured-art">
            <TrackCover track={featured} className="home-featured-cover" preferFull />
            <div className="home-featured-glow" aria-hidden />
          </div>
          <div className="home-featured-copy">
            <p className="home-eyebrow">Suggested for you</p>
            <h2 title={getTrackTitle(featured)}>{getTrackTitle(featured)}</h2>
            <button
              className="home-link"
              type="button"
              disabled={!featured.artist}
              onClick={() => featured.artist && onOpenArtist(featured.artist)}
            >
              {featured.artist || "Unknown artist"}
            </button>
            {featured.album && (
              <button
                className="home-link home-link-muted"
                type="button"
                onClick={() =>
                  onOpenAlbum(
                    featured.album,
                    featured.album_artist || featured.artist,
                  )
                }
              >
                {featured.album}
              </button>
            )}
            <button
              className="home-play-btn"
              type="button"
              onClick={playFeatured}
            >
              <BiPlay /> Play
            </button>
          </div>
        </section>
      )}

      {mixRow.length > 0 && (
        <section className="home-section">
          <div className="home-section-head">
            <h3>Mix for you</h3>
            <p>Random cuts from your collection</p>
          </div>
          <div className="home-card-row">
            {mixRow.map((track) => (
              <button
                key={`mix-${track.path}`}
                className="home-track-card"
                type="button"
                onClick={() =>
                  onPlayTrack(
                    track.path,
                    [track, ...suggestions.filter((t) => t.path !== track.path)],
                  )
                }
              >
                <div className="home-track-card-art">
                  <TrackCover track={track} className="home-card-cover" preferFull />
                  <span className="home-card-play" aria-hidden>
                    <BiPlay />
                  </span>
                </div>
                <span className="home-card-title">{getTrackTitle(track)}</span>
                <span className="home-card-meta">{track.artist || "Unknown"}</span>
              </button>
            ))}
          </div>
        </section>
      )}

      {albumPicks.length > 0 && (
        <section className="home-section">
          <div className="home-section-head">
            <h3>Albums to explore</h3>
            <p>Jump into a random record</p>
          </div>
          <div className="home-card-row">
            {albumPicks.map((album) => (
              <button
                key={`${album.name}-${album.album_artist ?? ""}`}
                className="home-track-card"
                type="button"
                onClick={() =>
                  onOpenAlbum(album.name, album.album_artist)
                }
              >
                <div className="home-track-card-art">
                  <AlbumCover album={album} className="home-card-cover" />
                </div>
                <span className="home-card-title">{album.name}</span>
                <span className="home-card-meta">
                  {album.album_artist || album.artist || "Various"}
                </span>
              </button>
            ))}
          </div>
        </section>
      )}

      {moreRow.length > 0 && (
        <section className="home-section home-section-grid">
          <div className="home-section-head">
            <h3>More to dig into</h3>
            <p>Another handful of random tracks</p>
          </div>
          <div className="home-suggest-grid">
            {moreRow.map((track) => (
              <button
                key={`more-${track.path}`}
                className="home-suggest-row"
                type="button"
                onClick={() =>
                  onPlayTrack(
                    track.path,
                    [track, ...suggestions.filter((t) => t.path !== track.path)],
                  )
                }
              >
                <TrackCover track={track} className="home-suggest-thumb" />
                <span className="home-suggest-text">
                  <span className="home-card-title">{getTrackTitle(track)}</span>
                  <span className="home-card-meta">
                    {track.artist || "Unknown"}
                    {track.album ? ` · ${track.album}` : ""}
                  </span>
                </span>
                <span className="home-suggest-play" aria-hidden>
                  <BiPlay />
                </span>
              </button>
            ))}
          </div>
        </section>
      )}
    </main>
  );
}
