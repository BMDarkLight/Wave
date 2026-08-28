import { useEffect, useMemo, useState } from "react";
import { BiPlay, BiMusic } from "react-icons/bi";
import {
  getHomeSuggestions,
  getPlaylistTracksById,
  getTrackFullCover,
  listAlbums,
  resolveCoverSrc,
} from "../utils/player";
import type { AlbumSummary, HomeSuggestions, Track } from "../utils/player";

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

/** Higher = richer tags / art. Used to bias Home suggestions when cold. */
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

type HomeCache = {
  libraryPlaylistId: string | null;
  suggestions: HomeSuggestions | null;
  fallbackTracks: Track[];
  fallbackAlbums: AlbumSummary[];
  seed: number;
};

let homeSessionCache: HomeCache | null = null;

const readHomeCache = (libraryPlaylistId: string | null) =>
  homeSessionCache?.libraryPlaylistId === libraryPlaylistId
    ? homeSessionCache
    : null;

const writeHomeCache = (cache: HomeCache) => {
  homeSessionCache = cache;
};

async function loadFallbackLibrary(libraryPlaylistId: string | null) {
  const [albumList, libraryTracks] = await Promise.all([
    listAlbums().catch(() => [] as AlbumSummary[]),
    libraryPlaylistId
      ? getPlaylistTracksById(libraryPlaylistId).catch(() => [] as Track[])
      : Promise.resolve([] as Track[]),
  ]);
  return { albums: albumList, tracks: libraryTracks };
}

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
  const initialCache = readHomeCache(libraryPlaylistId);
  const [suggestions, setSuggestions] = useState<HomeSuggestions | null>(
    initialCache?.suggestions ?? null,
  );
  const [fallbackTracks, setFallbackTracks] = useState<Track[]>(
    initialCache?.fallbackTracks ?? [],
  );
  const [fallbackAlbums, setFallbackAlbums] = useState<AlbumSummary[]>(
    initialCache?.fallbackAlbums ?? [],
  );
  const [loading, setLoading] = useState(!initialCache);
  const [seed] = useState(initialCache?.seed ?? 0);

  const loadSuggestions = async (nextSeed: number) => {
    try {
      const curated = await getHomeSuggestions();
      if (curated.featured || curated.mix.length > 0 || curated.more.length > 0) {
        setSuggestions(curated);
        writeHomeCache({
          libraryPlaylistId,
          suggestions: curated,
          fallbackTracks,
          fallbackAlbums,
          seed: nextSeed,
        });
        return;
      }
    } catch {
      // Fall through to metadata shuffle.
    }

    const { albums: albumList, tracks: libraryTracks } =
      await loadFallbackLibrary(libraryPlaylistId);
    setSuggestions(null);
    setFallbackAlbums(albumList);
    setFallbackTracks(libraryTracks);
    writeHomeCache({
      libraryPlaylistId,
      suggestions: null,
      fallbackTracks: libraryTracks,
      fallbackAlbums: albumList,
      seed: nextSeed,
    });
  };

  useEffect(() => {
    if (readHomeCache(libraryPlaylistId)) return;

    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        await loadSuggestions(0);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [libraryPlaylistId]);

  const coldSuggestions = useMemo(() => {
    void seed;
    return shufflePickPreferring(fallbackTracks, 18, trackMetadataScore, 10);
  }, [fallbackTracks, seed]);

  const featured = suggestions?.featured ?? coldSuggestions[0] ?? null;
  const mixRow = suggestions?.mix?.length
    ? suggestions.mix
    : coldSuggestions.slice(1, 9);
  const moreRow = suggestions?.more?.length
    ? suggestions.more
    : coldSuggestions.slice(9, 18);
  const discoveryRow = suggestions?.discovery ?? [];
  const [brokenCovers, setBrokenCovers] = useState<Set<string>>(new Set());
  const albumPicks = useMemo(() => {
    void seed;
    if (suggestions?.albums?.length) return suggestions.albums.slice(0, 8);
    return shufflePickPreferring(fallbackAlbums, 8, albumMetadataScore, 5);
  }, [suggestions, fallbackAlbums, seed]);

  const playQueue = useMemo(() => {
    const seen = new Set<string>();
    const out: Track[] = [];
    for (const track of [featured, ...mixRow, ...moreRow]) {
      if (!track || seen.has(track.path)) continue;
      seen.add(track.path);
      out.push(track);
    }
    return out;
  }, [featured, mixRow, moreRow]);

  const playFeatured = () => {
    if (!featured) return;
    onPlayTrack(featured.path, playQueue.length ? playQueue : [featured]);
  };

  const greeting = useMemo(() => {
    const hour = new Date().getHours();
    if (hour < 12) return "Good morning";
    if (hour < 18) return "Good afternoon";
    return "Good evening";
  }, []);

  const trackCount = fallbackTracks.length || playQueue.length;
  const curated = Boolean(suggestions?.curated);

  if (loading) {
    return (
      <main className="main-content home-page">
        <div className="empty-state">
          <div className="empty-icon">
            <span className="import-spinner" />
          </div>
          <h2>Finding music for you…</h2>
        </div>
      </main>
    );
  }

  if (!featured && mixRow.length === 0 && albumPicks.length === 0) {
    return (
      <main className="main-content home-page">
        <header className="home-header">
          <div>
            <p className="home-eyebrow">Wave</p>
            <h1>{greeting}</h1>
            <p className="home-sub">Your library is empty</p>
          </div>
        </header>
        <div className="empty-state">
          <div className="empty-icon">
            <BiMusic />
          </div>
          <h2>Nothing to suggest yet</h2>
          <p>Add music to your library to get personalized picks.</p>
          <button className="btn-primary" type="button" onClick={onOpenLibrary}>
            Open Library
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
            {curated
              ? "Curated from your listening history"
              : `Picked from ${trackCount.toLocaleString()} tracks in your library`}
          </p>
        </div>
      </header>

      {featured && (
        <section className="home-featured" aria-label="Featured track">
          <div className="home-featured-art">
            <TrackCover track={featured} className="home-featured-cover" preferFull />
            <div className="home-featured-glow" aria-hidden />
          </div>
          <div className="home-featured-copy">
            <p className="home-eyebrow">
              {curated ? "Because you listened" : "Suggested for you"}
            </p>
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
            <p>
              {curated
                ? "Neighbors from songs you finish — plus recent favorites"
                : "Random cuts from your collection"}
            </p>
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
                    [track, ...playQueue.filter((t) => t.path !== track.path)],
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
            <p>
              {curated
                ? "Records you’ve spent the most time with"
                : "Jump into a random record"}
            </p>
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
            <p>
              {curated
                ? "More from your listen graph"
                : "Another handful of random tracks"}
            </p>
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
                    [track, ...playQueue.filter((t) => t.path !== track.path)],
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

      {discoveryRow.length > 0 && (
        <section className="home-section" aria-label="Artists you might also like">
          <div className="home-section-head">
            <h3>You might also like</h3>
            <p>Similar artists based on what you listen to</p>
          </div>
          <div className="home-card-row">
            {discoveryRow.map((artist) => {
              const key = `${artist.name}-${artist.similar_to}`;
              const showCover = Boolean(artist.cover_url) && !brokenCovers.has(key);
              return (
                <div key={key} className="home-discovery-card">
                  <div className="home-discovery-art">
                    {showCover ? (
                      <img
                        src={artist.cover_url ?? undefined}
                        alt=""
                        loading="lazy"
                        onError={() =>
                          setBrokenCovers((prev) => {
                            const next = new Set(prev);
                            next.add(key);
                            return next;
                          })
                        }
                      />
                    ) : (
                      <BiMusic aria-hidden />
                    )}
                  </div>
                  <span className="home-card-title">{artist.name}</span>
                  <span className="home-card-meta">Similar to {artist.similar_to}</span>
                </div>
              );
            })}
          </div>
        </section>
      )}
    </main>
  );
}
