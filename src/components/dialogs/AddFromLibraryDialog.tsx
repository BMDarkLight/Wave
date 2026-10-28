/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import {
  BiCheck,
  BiChevronLeft,
  BiChevronRight,
  BiMinus,
  BiPlus,
  BiSearch,
  BiShuffle,
  BiX,
} from "react-icons/bi";
import Artwork from "../Artwork";
import { useDragSelect } from "../../hooks/useDragSelect";
import Select from "../Select";
import VirtualizedList from "../VirtualizedList";
import {
  formatListenDuration,
  getPlaylistTracksById,
  type PlaylistInfo,
  type Track,
} from "../../utils/player";
import { formatTime } from "../../utils/format";
import { isMobileLayout } from "../../utils/platform";
import {
  getTrackTitle,
  isLibraryPlaylistName,
  matchesQuery,
} from "../../utils/track";

type Tab = "songs" | "albums" | "artists" | "playlists" | "picked";

type SortKey = "title" | "artist" | "album" | "year" | "length" | "added";
type AddOrder = "picked" | "shuffle" | SortKey;

type Collection = {
  kind: "album" | "artist" | "playlist";
  key: string;
  name: string;
  sub: string;
  /** A playlist's songs are only fetched once it is opened or picked. */
  tracks: Track[] | null;
  cover: Track | null;
};

type Pick = { track: Track; rank: number };

const ROW_HEIGHT = 58;

const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: "title", label: "Title" },
  { value: "artist", label: "Artist" },
  { value: "album", label: "Album" },
  { value: "year", label: "Year" },
  { value: "length", label: "Length" },
  { value: "added", label: "Recently added" },
];

const ORDER_OPTIONS: { value: AddOrder; label: string }[] = [
  { value: "picked", label: "Order picked" },
  { value: "shuffle", label: "Shuffled" },
  ...SORT_OPTIONS,
];

const text = (a?: string | null, b?: string | null) =>
  (a ?? "").localeCompare(b ?? "", undefined, {
    sensitivity: "base",
    numeric: true,
  });

const albumOrder = (a: Track, b: Track) =>
  text(a.album, b.album) ||
  (a.disc_number ?? 0) - (b.disc_number ?? 0) ||
  (a.track_number ?? 0) - (b.track_number ?? 0) ||
  text(getTrackTitle(a), getTrackTitle(b));

const COMPARE: Record<SortKey, (a: Track, b: Track) => number> = {
  title: (a, b) => text(getTrackTitle(a), getTrackTitle(b)),
  artist: (a, b) => text(a.artist, b.artist) || albumOrder(a, b),
  album: (a, b) => albumOrder(a, b) || text(a.artist, b.artist),
  // Songs with no year go last rather than first.
  year: (a, b) => (a.year ?? 9999) - (b.year ?? 9999) || albumOrder(a, b),
  length: (a, b) => (a.duration_seconds ?? 0) - (b.duration_seconds ?? 0),
  added: (a, b) => b.indexed_at - a.indexed_at,
};

const albumArtistOf = (track: Track) => track.album_artist || track.artist;

const plural = (count: number, word: string) =>
  `${count} ${word}${count === 1 ? "" : "s"}`;

const coverOf = (tracks: Track[]) =>
  tracks.find((track) => track.cover_art_data_url) ?? tracks[0] ?? null;

/** Picks songs from the library, whole albums, artists or other playlists
 * included, and adds them to one playlist in a single go. */
export default function AddFromLibraryDialog({
  targetName,
  libraryPlaylistId,
  playlists,
  targetPlaylistId,
  existingPaths,
  adding,
  onClose,
  onPickFile,
  onAdd,
}: {
  targetName: string;
  libraryPlaylistId: string | null;
  playlists: PlaylistInfo[];
  targetPlaylistId: string;
  /** Songs the playlist already holds, which can't be picked again. */
  existingPaths: Set<string>;
  adding: boolean;
  onClose: () => void;
  onPickFile: () => void;
  onAdd: (paths: string[]) => void;
}) {
  const [library, setLibrary] = useState<Track[] | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("songs");
  const [openCollection, setOpenCollection] = useState<Collection | null>(null);
  const [sortKey, setSortKey] = useState<SortKey>("title");
  const [addOrder, setAddOrder] = useState<AddOrder>("picked");
  const [picks, setPicks] = useState<Map<string, Pick>>(() => new Map());
  const [playlistTracks, setPlaylistTracks] = useState<Record<string, Track[]>>(
    {},
  );
  // Where the last plain click landed, so shift-click can pick a run of rows.
  const rangeAnchor = useRef<{ list: string; index: number } | null>(null);

  useEffect(() => {
    if (!libraryPlaylistId) {
      setLibrary([]);
      return;
    }
    let cancelled = false;
    getPlaylistTracksById(libraryPlaylistId)
      .then((tracks) => {
        if (!cancelled) setLibrary(tracks);
      })
      .catch(() => {
        if (cancelled) return;
        setLibrary([]);
        setLoadError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [libraryPlaylistId]);

  const trimmed = query.trim();
  const allTracks = useMemo(() => library ?? [], [library]);

  const songs = useMemo(() => {
    const matching = trimmed
      ? allTracks.filter((track) =>
          matchesQuery(
            trimmed,
            track.title,
            track.name,
            track.artist,
            track.album,
            track.album_artist,
            track.genre,
            track.year?.toString(),
          ),
        )
      : allTracks;
    return [...matching].sort(COMPARE[sortKey]);
  }, [allTracks, trimmed, sortKey]);

  const albums = useMemo(() => {
    const groups = new Map<string, Track[]>();
    for (const track of allTracks) {
      if (!track.album) continue;
      const key = `${track.album}\u0000${albumArtistOf(track)}`;
      const group = groups.get(key);
      if (group) group.push(track);
      else groups.set(key, [track]);
    }
    const list: Collection[] = [];
    for (const [key, tracks] of groups) {
      const first = tracks[0];
      const artist = albumArtistOf(first);
      if (trimmed && !matchesQuery(trimmed, first.album, artist)) continue;
      const year = tracks.find((track) => track.year)?.year;
      list.push({
        kind: "album",
        key,
        name: first.album,
        sub: [artist, year, plural(tracks.length, "song")]
          .filter(Boolean)
          .join(" · "),
        tracks: [...tracks].sort(albumOrder),
        cover: coverOf(tracks),
      });
    }
    return list.sort((a, b) => text(a.name, b.name));
  }, [allTracks, trimmed]);

  const artists = useMemo(() => {
    const groups = new Map<string, Track[]>();
    for (const track of allTracks) {
      if (!track.artist) continue;
      const group = groups.get(track.artist);
      if (group) group.push(track);
      else groups.set(track.artist, [track]);
    }
    const list: Collection[] = [];
    for (const [name, tracks] of groups) {
      if (trimmed && !matchesQuery(trimmed, name)) continue;
      const albumCount = new Set(tracks.map((track) => track.album)).size;
      list.push({
        kind: "artist",
        key: name,
        name,
        sub: `${plural(albumCount, "album")} · ${plural(tracks.length, "song")}`,
        tracks: [...tracks].sort(COMPARE.year),
        cover: coverOf(tracks),
      });
    }
    return list.sort((a, b) => text(a.name, b.name));
  }, [allTracks, trimmed]);

  const otherPlaylists = useMemo(
    () =>
      playlists
        .filter(
          (p) =>
            p.id !== targetPlaylistId &&
            !isLibraryPlaylistName(p.name) &&
            (!trimmed || matchesQuery(trimmed, p.name)),
        )
        .map<Collection>((p) => {
          const tracks = playlistTracks[p.id] ?? null;
          return {
            kind: "playlist",
            key: p.id,
            name: p.name,
            sub: plural(tracks?.length ?? p.track_count, "song"),
            tracks,
            cover: tracks ? coverOf(tracks) : null,
          };
        }),
    [playlists, targetPlaylistId, trimmed, playlistTracks],
  );

  const pickedTracks = useMemo(() => {
    const list = [...picks.values()];
    if (addOrder === "picked") return list.map((pick) => pick.track);
    if (addOrder === "shuffle") {
      return list.sort((a, b) => a.rank - b.rank).map((pick) => pick.track);
    }
    return list.map((pick) => pick.track).sort(COMPARE[addOrder]);
  }, [picks, addOrder]);

  const pickedSeconds = useMemo(
    () =>
      pickedTracks.reduce(
        (total, track) => total + (track.duration_seconds ?? 0),
        0,
      ),
    [pickedTracks],
  );

  const canPick = (track: Track) => !existingPaths.has(track.path);

  const setPicked = (tracks: Track[], on: boolean) => {
    setPicks((prev) => {
      const next = new Map(prev);
      for (const track of tracks) {
        if (on) {
          if (!canPick(track) || next.has(track.path)) continue;
          // Picking order is kept by insertion; the rank is a random slot
          // for "Shuffled", drawn once so the list doesn't jump around.
          next.set(track.path, { track, rank: Math.random() });
        } else {
          next.delete(track.path);
        }
      }
      return next;
    });
  };

  const reshuffle = () => {
    setPicks((prev) => {
      const next = new Map<string, Pick>();
      for (const [path, pick] of prev) {
        next.set(path, { ...pick, rank: Math.random() });
      }
      return next;
    });
  };

  const onSongClick = (
    list: string,
    rows: Track[],
    index: number,
    event: MouseEvent,
  ) => {
    const track = rows[index];
    const turnOn = !picks.has(track.path);
    const anchor = rangeAnchor.current;
    if (event.shiftKey && anchor && anchor.list === list) {
      const from = Math.min(anchor.index, index);
      const to = Math.max(anchor.index, index);
      setPicked(rows.slice(from, to + 1), turnOn);
    } else {
      setPicked([track], turnOn);
    }
    rangeAnchor.current = { list, index };
  };

  // Press a song and slide over others to pick (or unpick) them all. The
  // slide starts from the picks as they were, so sliding back undoes it.
  const slideRef = useRef<{
    rows: Track[];
    before: Map<string, Pick>;
    on: boolean;
  } | null>(null);
  const rowsFor = (list: string) => {
    if (list === "songs") return songs;
    if (list === "picked") return pickedTracks;
    return openCollection?.tracks ?? [];
  };
  const dragSelect = useDragSelect({
    scrollSelector: ".song-picker-scroll",
    onStart: (list, index) => {
      const rows = rowsFor(list);
      slideRef.current = {
        rows,
        before: picks,
        on: !picks.has(rows[index]?.path),
      };
      rangeAnchor.current = { list, index };
    },
    onSpan: (anchor, current) => {
      const slide = slideRef.current;
      if (!slide) return;
      // In the order the slide passed over them, for "Order picked".
      const passed =
        current >= anchor
          ? slide.rows.slice(anchor, current + 1)
          : slide.rows.slice(current, anchor + 1).reverse();
      const next = new Map(slide.before);
      for (const track of passed) {
        if (!slide.on) next.delete(track.path);
        else if (canPick(track) && !next.has(track.path)) {
          next.set(track.path, { track, rank: Math.random() });
        }
      }
      setPicks(next);
    },
  });

  const loadPlaylist = async (collection: Collection) => {
    if (collection.tracks) return collection.tracks;
    const tracks = await getPlaylistTracksById(collection.key).catch(
      () => [] as Track[],
    );
    setPlaylistTracks((prev) => ({ ...prev, [collection.key]: tracks }));
    return tracks;
  };

  const pickState = (tracks: Track[] | null) => {
    const available = (tracks ?? []).filter(canPick);
    if (available.length === 0) return tracks ? "none-left" : "none";
    const picked = available.filter((track) => picks.has(track.path)).length;
    if (picked === 0) return "none";
    return picked === available.length ? "all" : "some";
  };

  const toggleCollection = async (collection: Collection) => {
    const tracks = await loadPlaylist(collection);
    setPicked(tracks, pickState(tracks) !== "all");
  };

  const openCollectionPage = async (collection: Collection) => {
    rangeAnchor.current = null;
    if (collection.kind === "playlist" && !collection.tracks) {
      const tracks = await loadPlaylist(collection);
      setOpenCollection({
        ...collection,
        tracks,
        cover: coverOf(tracks),
      });
      return;
    }
    setOpenCollection(collection);
  };

  const switchTab = (next: Tab) => {
    rangeAnchor.current = null;
    setOpenCollection(null);
    setTab(next);
  };

  const pickedCount = picks.size;
  const loading = library === null;

  const tabs: { id: Tab; label: string; count: number }[] = [
    { id: "songs", label: "Songs", count: songs.length },
    { id: "albums", label: "Albums", count: albums.length },
    { id: "artists", label: "Artists", count: artists.length },
    { id: "playlists", label: "Playlists", count: otherPlaylists.length },
    { id: "picked", label: "Picked", count: pickedCount },
  ];

  const songRow = (list: string, rows: Track[], index: number) => {
    const track = rows[index];
    const inPlaylist = !canPick(track);
    const picked = picks.has(track.path);
    // Unpicking by sliding in the Picked tab would pull rows out from under
    // the finger, so there a tap drops one song at a time.
    const slides = list !== "picked";
    return (
      <button
        type="button"
        role="option"
        aria-selected={picked}
        className={`song-picker-row${picked ? " is-picked" : ""}${dragSelect.isHolding(list, index) ? " is-holding" : ""}`}
        disabled={inPlaylist || adding}
        {...(slides ? dragSelect.rowProps(list, index) : {})}
        onClick={(event) => {
          if (dragSelect.consumeClick()) return;
          onSongClick(list, rows, index, event);
        }}
      >
        <span className="song-picker-check" aria-hidden>
          {picked && <BiCheck />}
        </span>
        <Artwork track={track} fallback="♪" className="song-picker-cover" />
        <span className="song-picker-meta">
          <span className="song-picker-title">{getTrackTitle(track)}</span>
          <span className="song-picker-sub">
            {[track.artist, track.album].filter(Boolean).join(" · ")}
          </span>
        </span>
        {inPlaylist ? (
          <span className="song-picker-tag">In playlist</span>
        ) : (
          <span className="song-picker-time">
            {track.duration_seconds ? formatTime(track.duration_seconds) : ""}
          </span>
        )}
      </button>
    );
  };

  const songList = (list: string, rows: Track[]) => (
    <VirtualizedList
      count={rows.length}
      estimateSize={ROW_HEIGHT}
      scrollSelector=".song-picker-scroll"
      className="song-picker-list"
    >
      {(index) => songRow(list, rows, index)}
    </VirtualizedList>
  );

  const collectionRow = (collection: Collection) => {
    const state = pickState(collection.tracks);
    return (
      <div
        key={collection.key}
        className={`song-picker-row is-collection${state === "all" ? " is-picked" : ""}`}
      >
        <button
          type="button"
          className="song-picker-open"
          onClick={() => void openCollectionPage(collection)}
          disabled={adding}
        >
          <Artwork
            track={collection.cover}
            fallback={collection.kind === "artist" ? "♫" : "♪"}
            className={`song-picker-cover${collection.kind === "artist" ? " is-round" : ""}`}
            alt={collection.name}
          />
          <span className="song-picker-meta">
            <span className="song-picker-title">{collection.name}</span>
            <span className="song-picker-sub">{collection.sub}</span>
          </span>
          <BiChevronRight className="song-picker-chevron" aria-hidden />
        </button>
        {state === "none-left" ? (
          <span className="song-picker-tag">In playlist</span>
        ) : (
          <button
            type="button"
            className={`song-picker-toggle is-${state}`}
            aria-pressed={state === "all"}
            onClick={() => void toggleCollection(collection)}
            disabled={adding}
            title={state === "all" ? "Unpick all" : "Pick all"}
          >
            {state === "all" ? (
              <BiCheck />
            ) : state === "some" ? (
              <BiMinus />
            ) : (
              <BiPlus />
            )}
          </button>
        )}
      </div>
    );
  };

  const collectionList = (rows: Collection[]) => (
    <VirtualizedList
      count={rows.length}
      estimateSize={ROW_HEIGHT}
      scrollSelector=".song-picker-scroll"
      className="song-picker-list"
    >
      {(index) => collectionRow(rows[index])}
    </VirtualizedList>
  );

  const empty = (message: ReactNode) => (
    <p className="song-picker-empty">{message}</p>
  );

  const pickAllButton = (rows: Track[]) => {
    const available = rows.filter(canPick);
    if (available.length === 0) return null;
    const allPicked = available.every((track) => picks.has(track.path));
    return (
      <button
        type="button"
        className="btn-ghost btn-sm"
        onClick={() => setPicked(available, !allPicked)}
        disabled={adding}
      >
        {allPicked ? "Unpick all" : `Pick all ${available.length}`}
      </button>
    );
  };

  let toolbar: ReactNode = null;
  let body: ReactNode;
  if (loading) {
    body = empty("Loading your library…");
  } else if (openCollection) {
    const rows = openCollection.tracks ?? [];
    toolbar = (
      <div className="song-picker-toolbar is-collection">
        <button
          type="button"
          className="song-picker-back"
          onClick={() => {
            rangeAnchor.current = null;
            setOpenCollection(null);
          }}
          title="Back"
        >
          <BiChevronLeft />
        </button>
        <span className="song-picker-toolbar-title">
          <strong>{openCollection.name}</strong>
          <span>{openCollection.sub}</span>
        </span>
        {pickAllButton(rows)}
      </div>
    );
    body =
      rows.length === 0
        ? empty("This playlist is empty.")
        : songList(`open:${openCollection.key}`, rows);
  } else if (tab === "songs") {
    toolbar = (
      <div className="song-picker-toolbar">
        <span className="song-picker-count">
          {plural(songs.length, "song")}
        </span>
        <Select
          className="song-picker-sort"
          aria-label="Sort songs"
          value={sortKey}
          options={SORT_OPTIONS}
          onChange={(value) => {
            rangeAnchor.current = null;
            setSortKey(value as SortKey);
          }}
        />
        {pickAllButton(songs)}
      </div>
    );
    body =
      songs.length > 0
        ? songList("songs", songs)
        : empty(
            allTracks.length === 0
              ? loadError
                ? "Couldn't load your library."
                : "Your library is empty. Scan a music folder into Library first, or pick files below."
              : `No songs match “${trimmed}”.`,
          );
  } else if (tab === "picked") {
    toolbar = (
      <div className="song-picker-toolbar">
        <span className="song-picker-count">
          {pickedCount > 0
            ? "Added in this order. Tap a song to drop it."
            : "Nothing picked yet."}
        </span>
        {pickedCount > 0 && (
          <button
            type="button"
            className="btn-ghost btn-sm"
            onClick={() => setPicks(new Map())}
            disabled={adding}
          >
            Clear
          </button>
        )}
      </div>
    );
    body =
      pickedCount > 0
        ? songList("picked", pickedTracks)
        : empty("Pick songs, albums, artists or playlists to add them here.");
  } else {
    const rows =
      tab === "albums" ? albums : tab === "artists" ? artists : otherPlaylists;
    const noun =
      tab === "albums" ? "album" : tab === "artists" ? "artist" : "playlist";
    body =
      rows.length > 0
        ? collectionList(rows)
        : empty(
            trimmed
              ? `No ${noun}s match “${trimmed}”.`
              : `No ${noun}s in your library yet.`,
          );
  }

  const viewKey = openCollection ? `open:${openCollection.key}` : tab;

  return (
    <div className="modal-backdrop song-picker-backdrop" onClick={onClose}>
      <div
        className="modal-dialog song-picker"
        role="dialog"
        aria-label={`Add songs to ${targetName}`}
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          // Escape steps out of an open album first, like back does.
          if (openCollection) {
            event.stopPropagation();
            setOpenCollection(null);
            return;
          }
          onClose();
        }}
      >
        <div className="modal-header">
          <h2 className="song-picker-heading">
            Add songs
            <span title={targetName}>to {targetName}</span>
          </h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>

        <label className="song-picker-search">
          <BiSearch aria-hidden />
          <input
            type="search"
            autoFocus={!isMobileLayout()}
            placeholder="Search songs, albums, artists, playlists"
            aria-label="Search your library"
            value={query}
            onChange={(event) => {
              rangeAnchor.current = null;
              setOpenCollection(null);
              setQuery(event.target.value);
            }}
            disabled={adding}
          />
        </label>

        <div className="song-picker-tabs" role="tablist">
          {tabs.map((item) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={tab === item.id && !openCollection}
              className={`song-picker-tab${tab === item.id ? " is-active" : ""}${item.id === "picked" ? " is-picked-tab" : ""}`}
              onClick={(event) => {
                // The row scrolls sideways on a phone; keep the chosen tab whole.
                event.currentTarget.scrollIntoView({
                  block: "nearest",
                  inline: "nearest",
                });
                switchTab(item.id);
              }}
            >
              {item.label}
              {!loading && <span>{item.count}</span>}
            </button>
          ))}
        </div>

        {toolbar}

        <div
          key={viewKey}
          className="song-picker-scroll"
          role="listbox"
          aria-multiselectable="true"
        >
          {body}
        </div>

        <div className="song-picker-footer">
          <div className="song-picker-order">
            <span className="song-picker-order-label">Add as</span>
            <Select
              aria-label="Order to add the songs in"
              value={addOrder}
              options={ORDER_OPTIONS}
              onChange={(value) => setAddOrder(value as AddOrder)}
              disabled={adding}
            />
            {addOrder === "shuffle" && (
              <button
                type="button"
                className="song-picker-reshuffle"
                onClick={reshuffle}
                disabled={adding || pickedCount < 2}
                title="Shuffle again"
              >
                <BiShuffle />
              </button>
            )}
          </div>
          <div className="song-picker-actions">
            <button
              className="btn-ghost"
              type="button"
              onClick={onPickFile}
              disabled={adding}
            >
              Pick files…
            </button>
            <span className="song-picker-summary">
              {pickedCount > 0 &&
                `${plural(pickedCount, "song")}${pickedSeconds > 0 ? ` · ${formatListenDuration(pickedSeconds)}` : ""}`}
            </span>
            <button
              className="btn-primary"
              type="button"
              onClick={() => onAdd(pickedTracks.map((track) => track.path))}
              disabled={adding || pickedCount === 0}
            >
              {adding
                ? "Adding…"
                : pickedCount > 0
                  ? `Add ${pickedCount}`
                  : "Add"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
