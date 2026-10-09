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
import { BiChevronRight } from "react-icons/bi";
import { lessMotion } from "../utils/appearance";
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
  loading,
  empty,
  children,
}: {
  title: string;
  count: number;
  loading: boolean;
  empty: string;
  children: ReactNode;
}) {
  return (
    <main className="main-content mobile-library-page">
      <div className="mlib-header">
        <h1>{title}</h1>
        {!loading && count > 0 && (
          <span className="mcol-count">{count.toLocaleString()}</span>
        )}
      </div>
      {loading ? (
        <p className="mcol-status">Loading…</p>
      ) : count === 0 ? (
        <p className="mcol-status">{empty}</p>
      ) : (
        children
      )}
    </main>
  );
}

export function MobileAlbumsPage({
  onOpenAlbum,
}: {
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
}) {
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);

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
      loading={loading}
      empty="No albums yet"
    >
      <div className="mcol-album-grid">
        {albums.map((album) => (
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
}: {
  onOpenArtist: (artist: string) => void;
}) {
  const [artists, setArtists] = useState<ArtistSummary[]>([]);
  const [albums, setAlbums] = useState<AlbumSummary[]>([]);
  const [loading, setLoading] = useState(true);

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
      loading={loading}
      empty="No artists yet"
    >
      <div className="mlib-list">
        {artists.map((artist, index) => (
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
