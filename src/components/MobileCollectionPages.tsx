/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// Every album and every artist in the library, reached from the Library tab
// on the narrow layout. Each entry opens the existing album or artist page.

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { BiChevronLeft, BiChevronRight, BiSearch } from "react-icons/bi";
import MobileFilterField from "./MobileFilterField";
import { lessMotion } from "../utils/appearance";
import { matchesQuery } from "../utils/track";
import {
  getTrackFullCover,
  listAlbums,
  listArtists,
  resolveCoverSrc,
  type AlbumSummary,
  type ArtistSummary,
} from "../utils/player";

const EMPTY: AlbumSummary[] = [];

const byName = (a: string, b: string) =>
  a.localeCompare(b, undefined, { sensitivity: "base" });

/** True once the element has come near the screen, and from then on. */
function useSeen(ref: RefObject<Element | null>): boolean {
  const [seen, setSeen] = useState(false);

  useEffect(() => {
    const element = ref.current;
    if (seen || !element) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
      },
      { rootMargin: "200px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref, seen]);

  return seen;
}

/**
 * The small thumbnail shows at once, then the full cover replaces it when
 * the album scrolls near the screen, so a long grid only decodes the covers
 * someone actually looks at.
 */
function AlbumThumb({ album }: { album: AlbumSummary }) {
  const ref = useRef<HTMLDivElement>(null);
  const seen = useSeen(ref);
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (!seen) return;
    let cancelled = false;
    void (async () => {
      const thumb = await resolveCoverSrc(album.cover_art_data_url);
      if (!cancelled && thumb) setSrc(thumb);
      if (!album.cover_track_path) return;
      const full = await getTrackFullCover(album.cover_track_path);
      if (!cancelled && full) setSrc(full);
    })();
    return () => {
      cancelled = true;
    };
  }, [seen, album.cover_art_data_url, album.cover_track_path]);

  return (
    <div className="mcol-album-cover" ref={ref} aria-hidden>
      {src ? (
        <img src={src} alt="" draggable={false} />
      ) : (
        album.name.slice(0, 1).toUpperCase()
      )}
    </div>
  );
}

/** How long each cover stays in an artist's avatar before the next. */
const AVATAR_COVER_MS = 3200;

/**
 * An artist's avatar cycles through their album covers. Rows start at
 * different moments so a list of avatars does not change in step. With
 * motion reduced or off it keeps to the first cover.
 */
function ArtistAvatar({
  name,
  albums,
  offset,
}: {
  name: string;
  albums: AlbumSummary[];
  offset: number;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const seen = useSeen(ref);
  const [covers, setCovers] = useState<string[]>([]);
  const [shown, setShown] = useState(0);

  useEffect(() => {
    if (!seen) return;
    let cancelled = false;
    void Promise.all(
      albums.map((album) => resolveCoverSrc(album.cover_art_data_url)),
    ).then((resolved) => {
      if (!cancelled)
        setCovers([
          // A remaster often shares the original's art; show it once.
          ...new Set(resolved.filter((src): src is string => !!src)),
        ]);
    });
    return () => {
      cancelled = true;
    };
  }, [seen, albums]);

  useEffect(() => {
    if (covers.length < 2 || lessMotion()) return;
    let timer: ReturnType<typeof setInterval> | undefined;
    const start = setTimeout(
      () => {
        setShown((index) => (index + 1) % covers.length);
        timer = setInterval(
          () => setShown((index) => (index + 1) % covers.length),
          AVATAR_COVER_MS,
        );
      },
      AVATAR_COVER_MS + (offset % 5) * 600,
    );
    return () => {
      clearTimeout(start);
      if (timer) clearInterval(timer);
    };
  }, [covers.length, offset]);

  return (
    <span className="mcol-artist-avatar" ref={ref} aria-hidden>
      {covers.length === 0
        ? name.slice(0, 1).toUpperCase()
        : covers.map((src, index) => (
            <img
              key={src}
              src={src}
              alt=""
              draggable={false}
              className={index === shown ? "is-shown" : undefined}
            />
          ))}
    </span>
  );
}

function CollectionShell({
  title,
  count,
  shown,
  loading,
  empty,
  searchPlaceholder,
  query,
  onQueryChange,
  onBack,
  children,
}: {
  title: string;
  /** Everything in the collection. */
  count: number;
  /** What is left after the search. */
  shown: number;
  loading: boolean;
  empty: string;
  searchPlaceholder: string;
  query: string;
  onQueryChange: (query: string) => void;
  onBack: () => void;
  children: ReactNode;
}) {
  const [searching, setSearching] = useState(false);

  const closeSearch = () => {
    onQueryChange("");
    setSearching(false);
  };

  return (
    <main className="main-content mobile-library-page">
      <div className="mlib-header mcol-header">
        <button
          className="mlib-icon-btn"
          onClick={onBack}
          type="button"
          title="Back"
          aria-label="Back to library"
        >
          <BiChevronLeft />
        </button>
        <h1>{title}</h1>
        {!loading && count > 0 && (
          <span className="mcol-count">
            {query.trim()
              ? `${shown.toLocaleString()} of ${count.toLocaleString()}`
              : count.toLocaleString()}
          </span>
        )}
        <button
          className={`mlib-icon-btn mcol-search-btn${searching ? " active" : ""}`}
          onClick={() => (searching ? closeSearch() : setSearching(true))}
          type="button"
          title={`Search ${title.toLowerCase()}`}
          aria-label={`Search ${title.toLowerCase()}`}
          aria-expanded={searching}
        >
          <BiSearch />
        </button>
      </div>
      <MobileFilterField
        open={searching}
        label={`Search ${title.toLowerCase()}`}
        placeholder={searchPlaceholder}
        query={query}
        onQueryChange={onQueryChange}
        onClose={closeSearch}
      />
      {loading ? (
        <p className="mcol-status">Loading…</p>
      ) : count === 0 ? (
        <p className="mcol-status">{empty}</p>
      ) : shown === 0 ? (
        <p className="mcol-status">Nothing matches “{query.trim()}”</p>
      ) : (
        children
      )}
    </main>
  );
}

export function MobileAlbumsPage({
  onOpenAlbum,
  onBack,
}: {
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onBack: () => void;
}) {
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const shown = useMemo(
    () =>
      albums.filter((album) =>
        matchesQuery(query, album.name, album.album_artist, album.artist),
      ),
    [albums, query],
  );

  useEffect(() => {
    let cancelled = false;
    listAlbums()
      .then((list) => {
        if (!cancelled)
          setAlbums([...list].sort((a, b) => byName(a.name, b.name)));
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <CollectionShell
      title="Albums"
      count={albums.length}
      shown={shown.length}
      loading={loading}
      empty="No albums yet"
      searchPlaceholder="Albums or artists"
      query={query}
      onQueryChange={setQuery}
      onBack={onBack}
    >
      <div className="mcol-album-grid">
        {shown.map((album) => (
          <button
            key={`${album.name}-${album.album_artist ?? ""}`}
            className="mcol-album"
            onClick={() => onOpenAlbum(album.name, album.album_artist)}
            type="button"
          >
            <AlbumThumb album={album} />
            <span className="mcol-album-name">{album.name}</span>
            <span className="mcol-album-artist">
              {album.album_artist || album.artist || "Various"}
            </span>
          </button>
        ))}
      </div>
    </CollectionShell>
  );
}

export function MobileArtistsPage({
  onOpenArtist,
  onBack,
}: {
  onOpenArtist: (artist: string) => void;
  onBack: () => void;
}) {
  const [artists, setArtists] = useState<ArtistSummary[]>([]);
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const shown = useMemo(
    () => artists.filter((artist) => matchesQuery(query, artist.name)),
    [artists, query],
  );

  useEffect(() => {
    listAlbums()
      .then(setAlbums)
      .catch(() => {});
  }, []);

  // An album counts for an artist when they are its album artist or the
  // artist on its tracks, so features and compilations still show up.
  const albumsByArtist = useMemo(() => {
    const map = new Map<string, AlbumSummary[]>();
    for (const album of albums) {
      const names = new Set(
        [album.album_artist, album.artist]
          .filter((name): name is string => !!name)
          .map((name) => name.trim().toLowerCase()),
      );
      for (const name of names) {
        map.set(name, [...(map.get(name) ?? []), album]);
      }
    }
    return map;
  }, [albums]);

  useEffect(() => {
    let cancelled = false;
    listArtists()
      .then((list) => {
        if (!cancelled)
          setArtists([...list].sort((a, b) => byName(a.name, b.name)));
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <CollectionShell
      title="Artists"
      count={artists.length}
      shown={shown.length}
      loading={loading}
      empty="No artists yet"
      searchPlaceholder="Artists"
      query={query}
      onQueryChange={setQuery}
      onBack={onBack}
    >
      <div className="mlib-list">
        {shown.map((artist, index) => (
          <button
            key={artist.name}
            className="mlib-row"
            onClick={() => onOpenArtist(artist.name)}
            type="button"
          >
            <ArtistAvatar
              name={artist.name}
              albums={
                albumsByArtist.get(artist.name.trim().toLowerCase()) ?? EMPTY
              }
              offset={index}
            />
            <span className="mcol-artist-text">
              <span className="mlib-row-label">{artist.name}</span>
              <span className="mcol-artist-meta">
                {artist.album_count === 1
                  ? "1 album"
                  : `${artist.album_count} albums`}
                {" · "}
                {artist.track_count === 1
                  ? "1 song"
                  : `${artist.track_count} songs`}
              </span>
            </span>
            <BiChevronRight className="mlib-row-chevron" aria-hidden />
          </button>
        ))}
      </div>
    </CollectionShell>
  );
}
