import {
  BiBarChartAlt2,
  BiCog,
  BiEditAlt,
  BiExport,
  BiHeart,
  BiHistory,
  BiHomeAlt2,
  BiImport,
  BiLibrary,
  BiPlus,
  BiSolidHeart,
  BiSync,
  BiTrash,
} from "react-icons/bi";
import trayTemplate from "../../assets/tray-template.svg";
import type { PlaylistInfo } from "../utils/player";

export type MainView =
  | "home"
  | "playlist"
  | "recently_played"
  | "most_played"
  | "settings";

export default function Sidebar({
  isScanningFolder,
  folderScanIsSync,
  isBrowsing,
  mainView,
  selectedPlaylistId,
  libraryPlaylist,
  favoritesPlaylist,
  userPlaylists,
  onGoHome,
  onGoRecentlyPlayed,
  onGoMostPlayed,
  onSelectPlaylist,
  onImportPlaylist,
  onCreatePlaylist,
  onSyncPlaylist,
  onExportPlaylist,
  onRenamePlaylist,
  onDeletePlaylist,
  onOpenSettings,
}: {
  isScanningFolder: boolean;
  folderScanIsSync: boolean;
  /** True while viewing an album/artist page (suppresses sidebar "active" states). */
  isBrowsing: boolean;
  mainView: MainView;
  selectedPlaylistId: string | null;
  libraryPlaylist: PlaylistInfo | null;
  favoritesPlaylist: PlaylistInfo | null;
  userPlaylists: PlaylistInfo[];
  onGoHome: () => void;
  onGoRecentlyPlayed: () => void;
  onGoMostPlayed: () => void;
  onSelectPlaylist: (id: string) => void;
  onImportPlaylist: () => void;
  onCreatePlaylist: () => void;
  onSyncPlaylist: (id: string) => void;
  onExportPlaylist: (id: string, name: string) => void;
  onRenamePlaylist: (id: string, name: string) => void;
  onDeletePlaylist: (id: string) => void;
  onOpenSettings: () => void;
}) {
  return (
    <aside className="sidebar">
      <div className="brand-mark">
        <img src={trayTemplate} alt="Wave" className="brand-logo" />
        {isScanningFolder ? (
          <span
            className="brand-sync-spinner"
            title={folderScanIsSync ? "Syncing folders…" : "Importing…"}
            aria-label={folderScanIsSync ? "Syncing folders" : "Importing"}
            role="status"
          />
        ) : null}
      </div>
      <div className="sidebar-pins">
        <button
          className={`sidebar-pin ${!isBrowsing && mainView === "home" ? "active" : ""}`}
          onClick={onGoHome}
          type="button"
        >
          <span className="sidebar-pin-label">Home</span>
          <span className="sidebar-pin-icon" aria-hidden>
            <BiHomeAlt2 />
          </span>
        </button>
        {libraryPlaylist && (
          <button
            className={`sidebar-pin ${!isBrowsing && mainView === "playlist" && selectedPlaylistId === libraryPlaylist.id ? "active" : ""}`}
            onClick={() => onSelectPlaylist(libraryPlaylist.id)}
            type="button"
          >
            <span className="sidebar-pin-label">Library</span>
            <span className="sidebar-pin-icon" aria-hidden>
              <BiLibrary />
            </span>
          </button>
        )}
        {favoritesPlaylist && (
          <button
            className={`sidebar-pin ${!isBrowsing && mainView === "playlist" && selectedPlaylistId === favoritesPlaylist.id ? "active" : ""}`}
            onClick={() => onSelectPlaylist(favoritesPlaylist.id)}
            type="button"
          >
            <span className="sidebar-pin-label">Favorites</span>
            {favoritesPlaylist.track_count > 0 && (
              <span className="sidebar-pin-count">
                {favoritesPlaylist.track_count}
              </span>
            )}
            <span className="sidebar-pin-icon" aria-hidden>
              {favoritesPlaylist.track_count > 0 ? (
                <BiSolidHeart />
              ) : (
                <BiHeart />
              )}
            </span>
          </button>
        )}
        <button
          className={`sidebar-pin ${!isBrowsing && mainView === "recently_played" ? "active" : ""}`}
          onClick={onGoRecentlyPlayed}
          type="button"
        >
          <span className="sidebar-pin-label">Recently Played</span>
          <span className="sidebar-pin-icon" aria-hidden>
            <BiHistory />
          </span>
        </button>
        <button
          className={`sidebar-pin ${!isBrowsing && mainView === "most_played" ? "active" : ""}`}
          onClick={onGoMostPlayed}
          type="button"
        >
          <span className="sidebar-pin-label">Most Played</span>
          <span className="sidebar-pin-icon" aria-hidden>
            <BiBarChartAlt2 />
          </span>
        </button>
      </div>
      <div className="playlist-section">
        <div className="playlist-section-header">
          <p>Playlists</p>
          <button
            className="playlist-add-btn"
            onClick={onImportPlaylist}
            type="button"
            title="Import playlist"
          >
            <BiImport />
          </button>
          <button
            className="playlist-add-btn"
            onClick={onCreatePlaylist}
            type="button"
            title="Create playlist"
          >
            <BiPlus />
          </button>
        </div>
        <div className="playlist-list">
          {userPlaylists.length === 0 ? (
            <div className="playlist-empty">
              <p>No playlists yet</p>
              <button
                className="btn-ghost btn-sm"
                onClick={onCreatePlaylist}
                type="button"
              >
                Create one
              </button>
            </div>
          ) : (
            userPlaylists.map((pl) => (
              <div
                key={pl.id}
                className={`playlist-item ${!isBrowsing && mainView === "playlist" && selectedPlaylistId === pl.id ? "active" : ""}`}
                onClick={() => onSelectPlaylist(pl.id)}
              >
                <span className="playlist-item-name" title={pl.name}>
                  {pl.sync_folder &&
                  !(
                    isScanningFolder &&
                    folderScanIsSync &&
                    selectedPlaylistId === pl.id
                  ) ? (
                    <BiSync
                      className="playlist-sync-icon"
                      title="Click to sync with folder"
                      aria-label="Click to sync with folder"
                      onClick={(e) => {
                        e.stopPropagation();
                        onSyncPlaylist(pl.id);
                      }}
                    />
                  ) : isScanningFolder &&
                    folderScanIsSync &&
                    pl.sync_folder &&
                    selectedPlaylistId === pl.id ? (
                    <BiSync
                      className="playlist-sync-icon playlist-sync-spin"
                      title="Syncing with folder"
                      aria-label="Syncing with folder"
                    />
                  ) : pl.sync_folder ? (
                    <BiSync
                      className="playlist-sync-icon"
                      title="Synced with a folder"
                      aria-label="Synced with a folder"
                    />
                  ) : null}
                  {pl.name}
                </span>
                <span className="playlist-item-count">{pl.track_count}</span>
                <div className="playlist-item-actions">
                  <button
                    className="playlist-export-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      onExportPlaylist(pl.id, pl.name);
                    }}
                    title={`Export`}
                    type="button"
                  >
                    <BiExport />
                  </button>
                  <button
                    className="playlist-rename-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      onRenamePlaylist(pl.id, pl.name);
                    }}
                    title="Rename playlist"
                    type="button"
                  >
                    <BiEditAlt />
                  </button>
                  <button
                    className="playlist-delete-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      onDeletePlaylist(pl.id);
                    }}
                    title="Delete playlist"
                    type="button"
                  >
                    <BiTrash />
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
      <button
        className={`sidebar-settings-btn${mainView === "settings" && !isBrowsing ? " active" : ""}`}
        onClick={onOpenSettings}
        type="button"
      >
        <BiCog /> Settings
      </button>
    </aside>
  );
}
