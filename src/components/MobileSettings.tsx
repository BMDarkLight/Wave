// Mobile-only Settings page: media source folders, playlist management,
// equalizer, crossfade, and audio output — all in one full-screen page.
import { useEffect, useState } from "react";
import { BiArrowBack } from "react-icons/bi";
import {
  BiFolderOpen,
  BiFolderMinus,
  BiImport,
  BiPlus,
  BiEditAlt,
  BiTrash,
  BiExport,
  BiSync,
  BiDevices,
  BiTimer,
  BiSliderAlt,
  BiCheck,
} from "react-icons/bi";
import {
  listMediaFolders,
  removeMediaFolder,
  listOutputDevices,
  getFileName,
  EQ_BAND_LABELS,
  EQ_PRESETS,
} from "../utils/player";
import type { EqSettings, PlaylistInfo } from "../utils/player";

const LIBRARY_PLAYLIST_NAME = "Library";
const isLibraryPlaylistName = (name?: string | null) =>
  name === LIBRARY_PLAYLIST_NAME || name === "All Local Files";

interface MobileSettingsProps {
  closing?: boolean;
  onClose: () => void;
  playlists: PlaylistInfo[];
  isScanningFolder: boolean;
  onCreatePlaylist: () => void;
  onImportPlaylist: () => void;
  onRenamePlaylist: (id: string, currentName: string) => void;
  onDeletePlaylist: (id: string) => void;
  onExportPlaylist: (id: string, name: string) => void;
  onSyncPlaylist: (id: string) => void;
  onAddMediaSource: () => Promise<void>;
  eqSettings: EqSettings;
  onEqEnabledChange: (enabled: boolean) => void;
  onEqBandChange: (index: number, gain: number) => void;
  onEqPreset: (id: string) => void;
  onEqReset: () => void;
  crossfadeDuration: number;
  onCrossfadeChange: (value: number) => void;
  currentOutputDevice: string;
  onSelectOutputDevice: (name: string) => Promise<void>;
}

export default function MobileSettings({
  closing = false,
  onClose,
  playlists,
  isScanningFolder,
  onCreatePlaylist,
  onImportPlaylist,
  onRenamePlaylist,
  onDeletePlaylist,
  onExportPlaylist,
  onSyncPlaylist,
  onAddMediaSource,
  eqSettings,
  onEqEnabledChange,
  onEqBandChange,
  onEqPreset,
  onEqReset,
  crossfadeDuration,
  onCrossfadeChange,
  currentOutputDevice,
  onSelectOutputDevice,
}: MobileSettingsProps) {
  const [mediaFolders, setMediaFolders] = useState<string[]>([]);
  const [addingSource, setAddingSource] = useState(false);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [entered, setEntered] = useState(false);

  const refreshMediaFolders = () => {
    listMediaFolders().then(setMediaFolders).catch(() => {});
  };

  useEffect(() => {
    refreshMediaFolders();
    listOutputDevices().then(setOutputDevices).catch(() => {});
  }, []);

  useEffect(() => {
    const id = requestAnimationFrame(() => {
      requestAnimationFrame(() => setEntered(true));
    });
    return () => cancelAnimationFrame(id);
  }, []);

  const libraryPlaylist = playlists.find((p) => isLibraryPlaylistName(p.name));

  const handleAddSource = async () => {
    setAddingSource(true);
    try {
      await onAddMediaSource();
    } finally {
      setAddingSource(false);
      refreshMediaFolders();
    }
  };

  const handleRemoveFolder = async (path: string) => {
    try {
      await removeMediaFolder(path);
    } finally {
      refreshMediaFolders();
    }
  };

  const orderedPlaylists = [...playlists].sort((a, b) => {
    const priority = [LIBRARY_PLAYLIST_NAME, "Favorites"];
    const ai = priority.indexOf(a.name);
    const bi = priority.indexOf(b.name);
    if (ai !== -1 || bi !== -1) return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    return a.name.localeCompare(b.name);
  });

  return (
    <div
      className={`mobile-settings-page${entered && !closing ? " mset-open" : ""}${closing ? " mset-closing" : ""}`}
    >
      <div className="mset-header">
        <button
          className="page-back-btn mset-back-btn"
          onClick={onClose}
          type="button"
          aria-label="Close settings"
        >
          <BiArrowBack />
        </button>
        <h1>Settings</h1>
      </div>

      <div className="mset-scroll">
        <section className="mset-section">
          <h2>Media Source Folders</h2>
          <div className="mset-card">
            <div className="mset-row">
              <div className="mset-row-text">
                <span className="mset-row-label">Library folder</span>
                <span className="mset-row-value">
                  {libraryPlaylist?.sync_folder
                    ? getFileName(libraryPlaylist.sync_folder)
                    : "Not set"}
                </span>
              </div>
              <div className="mset-row-actions">
                {libraryPlaylist?.sync_folder && (
                  <button
                    className="btn-ghost btn-sm"
                    onClick={() =>
                      libraryPlaylist && onSyncPlaylist(libraryPlaylist.id)
                    }
                    disabled={isScanningFolder}
                    type="button"
                  >
                    <BiSync /> Sync
                  </button>
                )}
                <button
                  className="btn-primary btn-sm"
                  onClick={() => void handleAddSource()}
                  disabled={addingSource || isScanningFolder}
                  type="button"
                >
                  <BiFolderOpen />{" "}
                  {libraryPlaylist?.sync_folder ? "Change" : "Choose"}
                </button>
              </div>
            </div>
          </div>

          {mediaFolders.length > 0 && (
            <div className="mset-card mset-folder-list">
              <p className="mset-hint">
                Folders you've added as sources. Removing one just forgets
                it — your music stays where it is.
              </p>
              {mediaFolders.map((folder) => (
                <div className="mset-row mset-folder-row" key={folder}>
                  <span className="mset-row-value" title={folder}>
                    {getFileName(folder)}
                  </span>
                  <button
                    className="mset-icon-btn"
                    onClick={() => void handleRemoveFolder(folder)}
                    type="button"
                    title="Forget this folder"
                    aria-label="Forget this folder"
                  >
                    <BiFolderMinus />
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className="mset-section">
          <div className="mset-section-header">
            <h2>Playlists</h2>
            <div className="mset-section-header-actions">
              <button
                className="mset-icon-btn"
                onClick={onImportPlaylist}
                type="button"
                title="Import playlist"
              >
                <BiImport />
              </button>
              <button
                className="mset-icon-btn"
                onClick={onCreatePlaylist}
                type="button"
                title="Create playlist"
              >
                <BiPlus />
              </button>
            </div>
          </div>
          <div className="mset-card mset-playlist-list">
            {orderedPlaylists.map((pl) => {
              const locked = isLibraryPlaylistName(pl.name) || pl.name === "Favorites";
              return (
                <div className="mset-row mset-playlist-row" key={pl.id}>
                  <div className="mset-row-text">
                    <span className="mset-row-label">
                      {pl.sync_folder && <BiSync className="mset-sync-dot" />}
                      {pl.name}
                    </span>
                    <span className="mset-row-value">
                      {pl.track_count} track{pl.track_count === 1 ? "" : "s"}
                    </span>
                  </div>
                  <div className="mset-row-actions">
                    {pl.sync_folder && (
                      <button
                        className="mset-icon-btn"
                        onClick={() => onSyncPlaylist(pl.id)}
                        disabled={isScanningFolder}
                        type="button"
                        title="Sync with folder"
                      >
                        <BiSync />
                      </button>
                    )}
                    <button
                      className="mset-icon-btn"
                      onClick={() => onExportPlaylist(pl.id, pl.name)}
                      type="button"
                      title="Export"
                    >
                      <BiExport />
                    </button>
                    {!locked && (
                      <>
                        <button
                          className="mset-icon-btn"
                          onClick={() => onRenamePlaylist(pl.id, pl.name)}
                          type="button"
                          title="Rename"
                        >
                          <BiEditAlt />
                        </button>
                        <button
                          className="mset-icon-btn mset-icon-btn-danger"
                          onClick={() => onDeletePlaylist(pl.id)}
                          type="button"
                          title="Delete"
                        >
                          <BiTrash />
                        </button>
                      </>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </section>

        <section className="mset-section">
          <div className="mset-section-header">
            <h2>
              <BiSliderAlt /> Equalizer
            </h2>
            <label className="eq-enable">
              <input
                type="checkbox"
                checked={eqSettings.enabled}
                onChange={(e) => onEqEnabledChange(e.target.checked)}
              />
              On
            </label>
          </div>
          <div className="mset-card mset-eq-card">
            <select
              className="eq-preset-select mset-eq-preset"
              value=""
              onChange={(e) => {
                if (e.target.value) onEqPreset(e.target.value);
              }}
              aria-label="EQ preset"
            >
              <option value="" disabled>
                Presets
              </option>
              {EQ_PRESETS.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.label}
                </option>
              ))}
            </select>
            <div
              className={`mset-eq-bands ${eqSettings.enabled ? "" : "disabled"}`}
            >
              {EQ_BAND_LABELS.map((label, index) => (
                <div className="eq-band" key={label}>
                  <span className="eq-band-gain">
                    {(eqSettings.bands[index] ?? 0) > 0 ? "+" : ""}
                    {(eqSettings.bands[index] ?? 0).toFixed(0)}
                  </span>
                  <input
                    type="range"
                    min={-12}
                    max={12}
                    step={0.5}
                    value={eqSettings.bands[index] ?? 0}
                    onChange={(e) =>
                      onEqBandChange(index, Number(e.target.value))
                    }
                    aria-label={`${label} Hz`}
                  />
                  <span className="eq-band-label">{label}</span>
                </div>
              ))}
            </div>
            <button className="btn-ghost btn-sm" onClick={onEqReset} type="button">
              Reset EQ
            </button>
          </div>
        </section>

        <section className="mset-section">
          <h2>
            <BiTimer /> Crossfade
          </h2>
          <div className="mset-card">
            <div className="mset-crossfade-row">
              <input
                type="range"
                min={0}
                max={8}
                step={0.5}
                value={crossfadeDuration}
                onChange={(e) => onCrossfadeChange(Number(e.target.value))}
                aria-label="Crossfade duration in seconds"
              />
              <span className="mset-crossfade-value">
                {crossfadeDuration === 0 ? "Off" : `${crossfadeDuration.toFixed(1)}s`}
              </span>
            </div>
            <p className="mset-hint">
              Smoothly blend the end of one track into the start of the next.
            </p>
          </div>
        </section>

        <section className="mset-section">
          <h2>
            <BiDevices /> Audio Output
          </h2>
          <div className="mset-card mset-device-list">
            {outputDevices.length === 0 ? (
              <p className="mset-hint">No output devices found.</p>
            ) : (
              outputDevices.map((name) => (
                <button
                  key={name}
                  className={`mset-row mset-device-row ${name === currentOutputDevice ? "active" : ""}`}
                  onClick={() => void onSelectOutputDevice(name)}
                  type="button"
                >
                  <span className="mset-row-value">{name}</span>
                  {name === currentOutputDevice && <BiCheck />}
                </button>
              ))
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
