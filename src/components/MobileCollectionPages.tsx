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

import { useEffect, useState, type ReactNode } from "react";
import { BiChevronRight } from "react-icons/bi";
import {
  listAlbums,
  listArtists,
  resolveCoverSrc,
  type AlbumSummary,
  type ArtistSummary,
} from "../utils/player";

const byName = (a: string, b: string) =>
  a.localeCompare(b, undefined, { sensitivity: "base" });

/** Thumbnail only: a grid of every album should not decode full covers. */
function AlbumThumb({ album }: { album: AlbumSummary }) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    if (album.cover_art_data_url) {
      void resolveCoverSrc(album.cover_art_data_url).then((resolved) => {
        if (!cancelled && resolved) setSrc(resolved);
      });
    }
    return () => {
      cancelled = true;
    };
  }, [album.cover_art_data_url]);

  return src ? (
    <img
      className="mcol-album-cover"
      src={src}
      alt=""
      loading="lazy"
      draggable={false}
    />
  ) : (
    <div className="mcol-album-cover" aria-hidden>
      {album.name.slice(0, 1).toUpperCase()}
    </div>
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
  const [loading, setLoading] = useState(true);

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
        {artists.map((artist) => (
          <button
            key={artist.name}
            className="mlib-row"
            onClick={() => onOpenArtist(artist.name)}
            type="button"
          >
            <span className="mcol-artist-avatar" aria-hidden>
              {artist.name.slice(0, 1).toUpperCase()}
            </span>
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
