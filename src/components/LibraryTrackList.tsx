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
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import {
  BiAlignLeft,
  BiChevronLeft,
  BiDotsHorizontalRounded,
  BiEditAlt,
  BiEraser,
  BiExport,
  BiHeart,
  BiImage,
  BiMinus,
  BiMusic,
  BiPause,
  BiPlay,
  BiPlus,
  BiSearch,
  BiSolidHeart,
  BiSync,
  BiTrash,
  BiX,
} from "react-icons/bi";
import Artwork from "./Artwork";
import VirtualizedList from "./VirtualizedList";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";
import MobileFilterField from "./MobileFilterField";
import { useBackLayer } from "../utils/backLayers";
import { formatTime } from "../utils/format";
import {
  LIBRARY_PLAYLIST_NAME,
  getTrackTitle,
  isLibraryPlaylistName,
  matchesQuery,
} from "../utils/track";
import type { PlaybackState, PlaylistInfo, Track } from "../utils/player";

export type SortColumn = "index" | "title" | "album";
export type SortDirection = "asc" | "desc" | "none";

export default function LibraryTrackList({
  mainSearchQuery,
  onMainSearchQueryChange,
  mainSearchOpen,
  onOpenMainSearch,
  onCloseMainSearch,
  mainSearchInputRef,
  mainSearchResultsSubtitle,
  mainSearchResultsPanel,
  selectedPlaylist,
  playlist,
  sortedPlaylist,
  isLoadingPlaylist,
  isScanningFolder,
  isImporting,
  isAddingTracks,
  importingPlaylistId,
  selectedPlaylistId,
  importedCount,
  playbackState,
  androidHost,
  addTrackBtnRef,
  trackGridCols,
  sortColumn,
  sortDirection,
  favoritePaths,
  menuTrackPath,
  onPlayPause,
  onAddFolderAndroid,
  onOpenAddFromLibrary,
  onOpenAddTrackMenu,
  onAddTrack,
  onClearPlaylist,
  onRenamePlaylist,
  onExportPlaylist,
  onDeletePlaylist,
  onBack,
  onSort,
  onResizeAlbumColumn,
  onSyncPlaylist,
  isCurrentTrack,
  onPlayTrack,
  onOpenArtist,
  onOpenAlbum,
  onOpenTrackContextMenu,
  onCloseTrackMenu,
  onRemoveFromPlaylist,
  onRemoveFromLibrary,
  onToggleFavorite,
}: {
  mainSearchQuery: string;
  onMainSearchQueryChange: (value: string) => void;
  mainSearchOpen: boolean;
  onOpenMainSearch: () => void;
  onCloseMainSearch: () => void;
  mainSearchInputRef: RefObject<HTMLInputElement | null>;
  mainSearchResultsSubtitle: string;
  mainSearchResultsPanel: ReactNode;
  selectedPlaylist: PlaylistInfo | null;
  playlist: Track[];
  sortedPlaylist: Track[];
  isLoadingPlaylist: boolean;
  isScanningFolder: boolean;
  isImporting: boolean;
  isAddingTracks: boolean;
  importingPlaylistId: string | null;
  selectedPlaylistId: string | null;
  importedCount: number;
  playbackState: PlaybackState;
  androidHost: boolean;
  addTrackBtnRef: RefObject<HTMLButtonElement | null>;
  trackGridCols: string;
  sortColumn: SortColumn;
  sortDirection: SortDirection;
  favoritePaths: Set<string>;
  menuTrackPath: string | null;
  onPlayPause: () => void;
  onAddFolderAndroid: () => void;
  onOpenAddFromLibrary: () => void;
  onOpenAddTrackMenu: () => void;
  onAddTrack: () => void;
  onClearPlaylist: () => void;
  onRenamePlaylist: (id: string, currentName: string) => void;
  onExportPlaylist: (id: string, name: string) => void;
  onDeletePlaylist: (id: string) => void;
  /** Back to the Library tab. Only shown on the phone layout. */
  onBack: () => void;
  onSort: (column: SortColumn) => void;
  onResizeAlbumColumn: (event: React.MouseEvent) => void;
  onSyncPlaylist: (playlistId: string) => void;
  isCurrentTrack: (track: Track) => boolean;
  onPlayTrack: (index: number) => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onOpenTrackContextMenu: (track: Track, anchor: ContextMenuAnchor) => void;
  onCloseTrackMenu: () => void;
  onRemoveFromPlaylist: (path: string) => void;
  onRemoveFromLibrary: (path: string) => void;
  onToggleFavorite: (path: string) => void;
}) {
  const [playlistMenu, setPlaylistMenu] = useState<ContextMenuAnchor | null>(
    null,
  );
  useBackLayer(playlistMenu !== null, () => setPlaylistMenu(null));
  // Library and Favorites are built in; only playlists the user made can be
  // renamed, exported or deleted from here.
  const ownPlaylist =
    selectedPlaylist &&
    !isLibraryPlaylistName(selectedPlaylist.name) &&
    selectedPlaylist.name !== "Favorites"
      ? selectedPlaylist
      : null;
  const canClear = playlist.length > 0 && !ownPlaylist?.sync_folder;
  const fromMenu = (action: () => void) => () => {
    setPlaylistMenu(null);
    action();
  };

  // The phone layout's search narrows the list in place. Each shown row
  // keeps its position in the full list so playing it starts from there.
  const [filtering, setFiltering] = useState(false);
  const [filter, setFilter] = useState("");
  const shownRows = useMemo(() => {
    const rows = sortedPlaylist.map((track, index) => ({ track, index }));
    if (!filter.trim()) return rows;
    return rows.filter(({ track }) =>
      matchesQuery(filter, getTrackTitle(track), track.artist, track.album),
    );
  }, [sortedPlaylist, filter]);
  const closeFilter = () => {
    setFilter("");
    setFiltering(false);
  };

  return (
    <main className="main-content playlist-page">
      <div className="hero-copy">
        <div className="hero-top">
          <button
            className="mlib-icon-btn playlist-back-btn"
            onClick={onBack}
            type="button"
            title="Back"
            aria-label="Back to library"
          >
            <BiChevronLeft />
          </button>
          <h1>
            {mainSearchQuery.trim()
              ? "Search"
              : (selectedPlaylist?.name ?? LIBRARY_PLAYLIST_NAME)}
          </h1>
          <div className="hero-actions">
            {!mainSearchQuery.trim() && (
              <>
                <button
                  className="big-play"
                  onClick={onPlayPause}
                  type="button"
                  title="Play or pause"
                >
                  {playbackState.is_playing ? <BiPause /> : <BiPlay />}
                </button>
                {selectedPlaylist?.name !== "Favorites" && (
                  <div className="add-track-wrap">
                    <button
                      ref={addTrackBtnRef}
                      className="btn-secondary"
                      onClick={() => {
                        if (androidHost) {
                          const isLibrary = isLibraryPlaylistName(
                            selectedPlaylist?.name,
                          );
                          if (isLibrary) onAddFolderAndroid();
                          else onOpenAddFromLibrary();
                          return;
                        }
                        onOpenAddTrackMenu();
                      }}
                      disabled={isAddingTracks}
                      type="button"
                      title={
                        androidHost
                          ? isLibraryPlaylistName(selectedPlaylist?.name)
                            ? "Scan media folder"
                            : "Add from library"
                          : "Add tracks"
                      }
                    >
                      <BiPlus />
                    </button>
                  </div>
                )}
              </>
            )}
            <div
              className={`hero-search-wrap${mainSearchOpen || mainSearchQuery ? " is-open" : ""}`}
            >
              {!(mainSearchOpen || mainSearchQuery) ? (
                <button
                  className="btn-secondary hero-search-btn"
                  type="button"
                  onClick={onOpenMainSearch}
                  title="Search library"
                  aria-label="Search library"
                >
                  <BiSearch />
                </button>
              ) : (
                <div className="library-search-bar">
                  <BiSearch className="library-search-icon" aria-hidden />
                  <input
                    ref={mainSearchInputRef}
                    className="library-search-input"
                    type="search"
                    placeholder="Search songs, artists, albums, lyrics…"
                    value={mainSearchQuery}
                    onChange={(e) => onMainSearchQueryChange(e.target.value)}
                    aria-label="Search library"
                    autoComplete="off"
                    spellCheck={false}
                  />
                  {mainSearchQuery ? (
                    <button
                      className="library-search-clear"
                      type="button"
                      onClick={() => onMainSearchQueryChange("")}
                      title="Clear search"
                      aria-label="Clear search"
                    >
                      <BiX />
                    </button>
                  ) : (
                    <button
                      className="library-search-clear"
                      type="button"
                      onClick={onCloseMainSearch}
                      title="Close search"
                      aria-label="Close search"
                    >
                      <BiX />
                    </button>
                  )}
                </div>
              )}
            </div>
            {!mainSearchQuery.trim() && playlist.length > 0 && (
              <button
                className={`btn-secondary playlist-filter-btn${filtering ? " active" : ""}`}
                onClick={() => (filtering ? closeFilter() : setFiltering(true))}
                type="button"
                title="Search this playlist"
                aria-label="Search this playlist"
                aria-expanded={filtering}
              >
                <BiSearch />
              </button>
            )}
            {!mainSearchQuery.trim() && ownPlaylist && (
              <button
                className="btn-secondary"
                onClick={(event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  setPlaylistMenu({
                    top: rect.bottom + 6,
                    left: rect.left,
                    flipAbove: rect.top - 6,
                  });
                }}
                type="button"
                title="Playlist options"
                aria-label="Playlist options"
                aria-haspopup="menu"
                aria-expanded={playlistMenu !== null}
              >
                <BiDotsHorizontalRounded />
              </button>
            )}
            {playlistMenu && ownPlaylist && (
              <ContextMenu
                anchor={playlistMenu}
                onClose={() => setPlaylistMenu(null)}
              >
                <button
                  type="button"
                  role="menuitem"
                  onClick={fromMenu(() =>
                    onRenamePlaylist(ownPlaylist.id, ownPlaylist.name),
                  )}
                >
                  <BiEditAlt /> Rename
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={fromMenu(() =>
                    onExportPlaylist(ownPlaylist.id, ownPlaylist.name),
                  )}
                >
                  <BiExport /> Export
                </button>
                {canClear && (
                  <button
                    type="button"
                    role="menuitem"
                    onClick={fromMenu(onClearPlaylist)}
                  >
                    <BiEraser /> Clear
                  </button>
                )}
                <button
                  type="button"
                  role="menuitem"
                  className="delete-action"
                  onClick={fromMenu(() => onDeletePlaylist(ownPlaylist.id))}
                >
                  <BiTrash /> Delete
                </button>
              </ContextMenu>
            )}
          </div>
        </div>
        <p>
          {mainSearchQuery.trim()
            ? mainSearchResultsSubtitle
            : playlist.length
              ? filter.trim()
                ? `${shownRows.length} of ${playlist.length} tracks`
                : `${playlist.length} tracks in this playlist`
              : isLoadingPlaylist
                ? "Loading tracks…"
                : "No tracks in this playlist"}
          {!mainSearchQuery.trim() &&
          ((isScanningFolder &&
            (selectedPlaylist?.sync_folder ||
              isLibraryPlaylistName(selectedPlaylist?.name))) ||
            (isImporting && selectedPlaylist?.sync_folder)) ? (
            <>
              {" · "}
              <span className="playlist-sync-badge playlist-sync-badge-active">
                <BiSync className="playlist-sync-spin" /> Syncing…
              </span>
            </>
          ) : !mainSearchQuery.trim() && selectedPlaylist?.sync_folder ? (
            <span className="playlist-synced-note">
              {" · "}
              <span
                className="playlist-sync-badge"
                title={selectedPlaylist.sync_folder}
                onClick={() => onSyncPlaylist(selectedPlaylist.id)}
                style={{ cursor: "pointer" }}
              >
                <BiSync /> Synced folder
              </span>
            </span>
          ) : null}
        </p>
      </div>

      <MobileFilterField
        open={filtering && !mainSearchQuery.trim()}
        label="Search this playlist"
        placeholder="Songs, artists or albums"
        query={filter}
        onQueryChange={setFilter}
        onClose={closeFilter}
      />

      <section className="playlist-container">
        {mainSearchQuery.trim() ? (
          mainSearchResultsPanel
        ) : playlist.length === 0 && isLoadingPlaylist ? (
          <div className="empty-state">
            <div className="empty-icon">
              <span className="import-spinner" />
            </div>
            <h2>Loading…</h2>
          </div>
        ) : importingPlaylistId != null &&
          selectedPlaylistId === importingPlaylistId ? (
          <div className="empty-state">
            <div className="empty-icon">
              <span className="import-spinner" />
            </div>
            <h2>
              Importing songs
              {importedCount > 0 ? ` (${importedCount} added)` : ""}…
            </h2>
            <p className="import-subtitle">
              Your songs will appear here as they are added.
            </p>
          </div>
        ) : playlist.length > 0 && shownRows.length === 0 ? (
          <p className="mcol-status">Nothing matches “{filter.trim()}”</p>
        ) : playlist.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">
              <BiMusic />
            </div>
            <h2>Your playlist is empty</h2>
            {!isLibraryPlaylistName(selectedPlaylist?.name) &&
              selectedPlaylist?.name !== "Favorites" && (
                <button
                  className="btn-primary"
                  onClick={() => {
                    if (androidHost) onOpenAddFromLibrary();
                    else onAddTrack();
                  }}
                  disabled={isAddingTracks}
                  type="button"
                >
                  {androidHost ? "Add from library" : "Add your first track"}
                </button>
              )}
          </div>
        ) : (
          <div
            className="track-list"
            style={{ "--track-grid": trackGridCols } as CSSProperties}
          >
            <div className="track-list-header">
              <div
                className="track-col-index sort-header"
                onClick={() => onSort("index")}
              >
                #
                {sortColumn === "index" && sortDirection !== "none"
                  ? sortDirection === "asc"
                    ? " ▲"
                    : " ▼"
                  : ""}
              </div>
              <div
                className="track-title-cell sort-header"
                onClick={() => onSort("title")}
              >
                Title
                {sortColumn === "title" && sortDirection !== "none"
                  ? sortDirection === "asc"
                    ? " ▲"
                    : " ▼"
                  : ""}
                <div
                  className="resize-handle"
                  onMouseDown={onResizeAlbumColumn}
                  onClick={(e) => e.stopPropagation()}
                  title="Resize columns"
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize title and album columns"
                />
              </div>
              <div
                className="track-album sort-header"
                onClick={() => onSort("album")}
              >
                Album
                {sortColumn === "album" && sortDirection !== "none"
                  ? sortDirection === "asc"
                    ? " ▲"
                    : " ▼"
                  : ""}
              </div>
              <div className="track-duration track-duration-header">
                Duration
              </div>
            </div>
            <VirtualizedList
              count={shownRows.length}
              estimateSize={64}
              className="track-list-virtual"
            >
              {(row) => {
                const shownRow = shownRows[row];
                if (!shownRow) return null;
                const { track, index } = shownRow;
                return (
                  <div
                    key={track.id}
                    className={`track-item ${isCurrentTrack(track) ? "active" : ""}`}
                    onClick={() => onPlayTrack(index)}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      onOpenTrackContextMenu(track, {
                        top: event.clientY,
                        left: event.clientX,
                        flipAbove: event.clientY,
                      });
                    }}
                  >
                    <div className="track-col-index">
                      {isCurrentTrack(track) && playbackState.is_playing ? (
                        <span className="mini-bars">
                          <i />
                          <i />
                          <i />
                        </span>
                      ) : (
                        index + 1
                      )}
                    </div>
                    <div className="track-title-cell">
                      <span className="track-thumb-wrap">
                        <Artwork
                          track={track}
                          fallback={getTrackTitle(track)
                            .slice(0, 1)
                            .toUpperCase()}
                          className="track-thumb"
                        />
                        {isCurrentTrack(track) && playbackState.is_playing && (
                          <span className="track-thumb-playing" aria-hidden>
                            <span className="mini-bars">
                              <i />
                              <i />
                              <i />
                            </span>
                          </span>
                        )}
                      </span>
                      <div>
                        <div className="track-name">{getTrackTitle(track)}</div>
                        <div className="track-meta">
                          <button
                            className="track-meta-link"
                            onClick={(e) => {
                              e.stopPropagation();
                              if (window.innerWidth <= 900) return;
                              onOpenArtist(track.artist);
                            }}
                            type="button"
                          >
                            {track.artist}
                          </button>
                          {(track.lyrics ||
                            track.cover_art_source === "cover-art-archive") && (
                            <span className="track-meta-icons">
                              {track.lyrics ? (
                                <span
                                  className="track-meta-icon-wrap"
                                  title="Has lyrics"
                                  aria-label="Has lyrics"
                                >
                                  <BiAlignLeft
                                    className="track-meta-icon"
                                    aria-hidden
                                  />
                                </span>
                              ) : null}
                              {track.cover_art_source ===
                              "cover-art-archive" ? (
                                <span
                                  className="track-meta-icon-wrap"
                                  title="Online cover"
                                  aria-label="Online cover"
                                >
                                  <BiImage
                                    className="track-meta-icon"
                                    aria-hidden
                                  />
                                </span>
                              ) : null}
                            </span>
                          )}
                        </div>
                      </div>
                    </div>
                    <div
                      className="track-album"
                      onClick={(e) => {
                        e.stopPropagation();
                        if (window.innerWidth <= 900) return;
                        onOpenAlbum(
                          track.album,
                          track.album_artist || track.artist,
                        );
                      }}
                    >
                      {track.album}
                    </div>
                    <div className="track-duration">
                      {formatTime(track.duration_seconds)}
                    </div>
                    <div className="track-actions-cell">
                      <div className="track-actions-hover">
                        <button
                          className="track-action-btn"
                          onClick={(event) => {
                            event.stopPropagation();
                            if (menuTrackPath === track.path) {
                              onCloseTrackMenu();
                            } else {
                              const rect =
                                event.currentTarget.getBoundingClientRect();
                              onOpenTrackContextMenu(track, {
                                top: rect.bottom + 4,
                                flipAbove: rect.top - 4,
                                right: window.innerWidth - rect.right,
                              });
                            }
                          }}
                          title="More"
                          type="button"
                        >
                          <BiDotsHorizontalRounded />
                        </button>
                        {!isLibraryPlaylistName(selectedPlaylist?.name) && (
                          <button
                            className="track-action-btn track-remove-action"
                            onClick={(event) => {
                              event.stopPropagation();
                              onRemoveFromPlaylist(track.path);
                            }}
                            title="Remove from playlist"
                            type="button"
                          >
                            <BiMinus />
                          </button>
                        )}
                        <button
                          className="track-action-btn track-remove-action"
                          onClick={(event) => {
                            event.stopPropagation();
                            onRemoveFromLibrary(track.path);
                          }}
                          title="Remove from library"
                          type="button"
                        >
                          <BiTrash />
                        </button>
                      </div>
                      <button
                        className={`track-action-btn favorite-btn ${favoritePaths.has(track.path) ? "active" : ""}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          onToggleFavorite(track.path);
                        }}
                        title={
                          favoritePaths.has(track.path)
                            ? "Remove from Favorites"
                            : "Add to Favorites"
                        }
                        type="button"
                      >
                        {favoritePaths.has(track.path) ? (
                          <BiSolidHeart />
                        ) : (
                          <BiHeart />
                        )}
                      </button>
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
