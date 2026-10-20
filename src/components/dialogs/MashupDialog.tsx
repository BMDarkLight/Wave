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
import { BiX } from "react-icons/bi";
import {
  listAlbums,
  type AlbumSummary,
  type PlaylistInfo,
} from "../../utils/player";
import { isLibraryPlaylistName, matchesQuery } from "../../utils/track";
import { mashSourceKey, type MashSource } from "../../hooks/usePlaylistMashing";

const songCount = (count: number) => `${count} song${count === 1 ? "" : "s"}`;

/** Pick the albums and playlists to mash together with `start`, the one
 * whose page the dialog was opened from. */
export default function MashupDialog({
  start,
  playlists,
  onClose,
  onMash,
}: {
  start: MashSource;
  playlists: PlaylistInfo[];
  onClose: () => void;
  onMash: (sources: MashSource[]) => void;
}) {
  const [tab, setTab] = useState<MashSource["kind"]>(start.kind);
  const [query, setQuery] = useState("");
  const [albums, setAlbums] = useState<AlbumSummary[] | null>(null);
  const [picked, setPicked] = useState<Map<string, MashSource>>(
    () => new Map([[mashSourceKey(start), start]]),
  );

  useEffect(() => {
    listAlbums()
      .then(setAlbums)
      .catch(() => setAlbums([]));
  }, []);

  // The Library playlist already holds every song, so it has nothing to add.
  const rows = useMemo(() => {
    if (tab === "playlist") {
      return playlists
        .filter((p) => !isLibraryPlaylistName(p.name))
        .filter((p) => matchesQuery(query, p.name))
        .map((p) => ({
          source: { kind: "playlist", id: p.id, name: p.name } as MashSource,
          title: p.name,
          sub: songCount(p.track_count),
        }));
    }
    return (albums ?? [])
      .filter((a) => matchesQuery(query, a.name, a.album_artist, a.artist))
      .map((a) => ({
        source: {
          kind: "album",
          name: a.name,
          albumArtist: a.album_artist,
        } as MashSource,
        title: a.name,
        sub: [a.album_artist || a.artist, songCount(a.track_count)]
          .filter(Boolean)
          .join(" · "),
      }));
  }, [tab, query, playlists, albums]);

  const toggle = (source: MashSource) =>
    setPicked((current) => {
      const next = new Map(current);
      const key = mashSourceKey(source);
      if (next.has(key)) next.delete(key);
      else next.set(key, source);
      return next;
    });

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog library-picker-dialog mashup-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>Mash up</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>
        <p className="couple-dialog-hint">
          Pick the albums and playlists to mash together. Their songs are
          shuffled into one queue.
        </p>
        <div
          className="couple-order mashup-tabs"
          role="tablist"
          aria-label="Mash from"
        >
          <button
            type="button"
            role="tab"
            aria-selected={tab === "playlist"}
            className={tab === "playlist" ? "active" : ""}
            onClick={() => setTab("playlist")}
          >
            Playlists
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "album"}
            className={tab === "album" ? "active" : ""}
            onClick={() => setTab("album")}
          >
            Albums
          </button>
        </div>
        <input
          className="modal-input"
          type="search"
          autoFocus
          placeholder={
            tab === "playlist" ? "Filter playlists" : "Album or artist"
          }
          aria-label={tab === "playlist" ? "Filter playlists" : "Filter albums"}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <div
          className="library-picker-results"
          role="listbox"
          aria-multiselectable
        >
          {tab === "album" && albums === null ? (
            <p className="library-picker-empty">Loading albums…</p>
          ) : rows.length === 0 ? (
            <p className="library-picker-empty">
              {tab === "playlist"
                ? "No matching playlists."
                : "No matching albums."}
            </p>
          ) : (
            rows.map(({ source, title, sub }) => {
              const key = mashSourceKey(source);
              const isSelected = picked.has(key);
              return (
                <button
                  key={key}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  className={`library-picker-row${isSelected ? " selected" : ""}`}
                  onClick={() => toggle(source)}
                >
                  <span className="library-picker-check" aria-hidden>
                    {isSelected ? "✓" : ""}
                  </span>
                  <span className="library-picker-meta">
                    <span className="library-picker-title">{title}</span>
                    <span className="library-picker-sub">{sub}</span>
                  </span>
                </button>
              );
            })
          )}
        </div>
        <div className="modal-actions">
          <span className="mashup-picked">{picked.size} picked</span>
          <button className="btn-ghost" type="button" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn-primary"
            type="button"
            onClick={() => onMash([...picked.values()])}
            disabled={picked.size < 2}
            title={
              picked.size < 2 ? "Pick at least two to mash together" : undefined
            }
          >
            Mash
          </button>
        </div>
      </div>
    </div>
  );
}
