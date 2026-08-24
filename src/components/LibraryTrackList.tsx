import type { CSSProperties, ReactNode, RefObject } from "react";
import {
  BiAlignLeft,
  BiDotsHorizontalRounded,
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
import type { ContextMenuAnchor } from "./ContextMenu";
import { formatTime } from "../utils/format";
import { LIBRARY_PLAYLIST_NAME, getTrackTitle, isLibraryPlaylistName } from "../utils/track";
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
  onSort: (column: SortColumn) => void;
  onResizeAlbumColumn: (event: React.MouseEvent) => void;
  onSyncPlaylist: (playlistId: string) => void;
  isCurrentTrack: (track: Track) => boolean;
  onPlayTrack: (index: number) => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onOpenTrackContextMenu: (path: string, anchor: ContextMenuAnchor) => void;
  onCloseTrackMenu: () => void;
  onRemoveFromPlaylist: (path: string) => void;
  onRemoveFromLibrary: (path: string) => void;
  onToggleFavorite: (path: string) => void;
}) {
  return (
    <main className="main-content">
      <div className="hero-copy">
        <div className="hero-top">
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
            {!mainSearchQuery.trim() &&
              playlist.length > 0 &&
              !isLibraryPlaylistName(selectedPlaylist?.name) &&
              selectedPlaylist?.name !== "Favorites" &&
              !selectedPlaylist?.sync_folder && (
                <button
                  className="btn-ghost"
                  onClick={onClearPlaylist}
                  type="button"
                >
                  Clear
                </button>
              )}
          </div>
        </div>
        <p>
          {mainSearchQuery.trim()
            ? mainSearchResultsSubtitle
            : playlist.length
              ? `${playlist.length} tracks in this playlist`
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
            <>
              {" · "}
              <span
                className="playlist-sync-badge"
                title={selectedPlaylist.sync_folder}
                onClick={() => onSyncPlaylist(selectedPlaylist.id)}
                style={{ cursor: "pointer" }}
              >
                <BiSync /> Synced folder
              </span>
            </>
          ) : null}
        </p>
      </div>

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
              count={sortedPlaylist.length}
              estimateSize={
                typeof window !== "undefined" && window.innerWidth <= 900
                  ? 58
                  : 64
              }
              className="track-list-virtual"
            >
              {(index) => {
                const track = sortedPlaylist[index];
                if (!track) return null;
                return (
                  <div
                    key={track.id}
                    className={`track-item ${isCurrentTrack(track) ? "active" : ""}`}
                    onClick={() => onPlayTrack(index)}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      onOpenTrackContextMenu(track.path, {
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
                      <Artwork
                        track={track}
                        fallback={getTrackTitle(track).slice(0, 1).toUpperCase()}
                        className="track-thumb"
                      />
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
                                  <BiAlignLeft className="track-meta-icon" aria-hidden />
                                </span>
                              ) : null}
                              {track.cover_art_source === "cover-art-archive" ? (
                                <span
                                  className="track-meta-icon-wrap"
                                  title="Online cover"
                                  aria-label="Online cover"
                                >
                                  <BiImage className="track-meta-icon" aria-hidden />
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
                        onOpenAlbum(track.album, track.album_artist || track.artist);
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
                              onOpenTrackContextMenu(track.path, {
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
