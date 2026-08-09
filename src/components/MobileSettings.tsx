// Settings page: media source folders, playlist management, equalizer,
// crossfade, audio output (desktop/non-Android), and app reset.
import {
  useEffect,
  useRef,
  useState,
  type InputHTMLAttributes,
  type PointerEvent as ReactPointerEvent,
} from "react";
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
  BiErrorCircle,
  BiReset,
} from "react-icons/bi";
import {
  listMediaFolders,
  removeMediaFolder,
  listOutputDevices,
  getFileName,
  getListeningStats,
  formatListenDuration,
  EQ_BAND_LABELS,
  EQ_PRESETS,
} from "../utils/player";
import type { EqSettings, ListeningStats, PlaylistInfo } from "../utils/player";
import { isAndroid } from "../utils/platform";

const LIBRARY_PLAYLIST_NAME = "Library";
const isLibraryPlaylistName = (name?: string | null) =>
  name === LIBRARY_PLAYLIST_NAME || name === "All Local Files";

const getTrackTitle = (track: { title?: string; name?: string }) =>
  track.title || track.name || "Unknown";

function ListenRankList({
  title,
  items,
}: {
  title: string;
  items: { name: string; listen_seconds: number; play_count: number }[];
}) {
  if (items.length === 0) return null;
  return (
    <div className="mset-listen-group">
      <h3 className="mset-listen-group-title">{title}</h3>
      <ul className="mset-listen-rank">
        {items.map((item, index) => (
          <li key={`${title}-${item.name}`} className="mset-listen-rank-row">
            <span className="mset-listen-rank-index">{index + 1}</span>
            <span className="mset-listen-rank-name" title={item.name}>
              {item.name}
            </span>
            <span className="mset-listen-rank-meta">
              {formatListenDuration(item.listen_seconds)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * Range input that ignores accidental drags while the settings page scrolls.
 * Arms only after a short still press (or stays inert if the page moved).
 */
function ScrollSafeRange({
  value,
  onValueChange,
  className,
  ...rest
}: Omit<InputHTMLAttributes<HTMLInputElement>, "onChange" | "type" | "value"> & {
  value: number;
  onValueChange: (value: number) => void;
}) {
  const armedRef = useRef(false);
  const [armed, setArmed] = useState(false);
  const startRef = useRef<{
    x: number;
    y: number;
    scrollTop: number;
  } | null>(null);
  const armTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const clearArmTimer = () => {
    if (armTimerRef.current != null) {
      clearTimeout(armTimerRef.current);
      armTimerRef.current = null;
    }
  };

  const scrollParentOf = (el: HTMLElement | null) =>
    el?.closest(".mset-scroll") as HTMLElement | null;

  const disarm = () => {
    clearArmTimer();
    armedRef.current = false;
    setArmed(false);
    startRef.current = null;
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLInputElement>) => {
    disarm();
    const scroll = scrollParentOf(e.currentTarget);
    startRef.current = {
      x: e.clientX,
      y: e.clientY,
      scrollTop: scroll?.scrollTop ?? 0,
    };
    armTimerRef.current = setTimeout(() => {
      const scrollNow = scrollParentOf(inputRef.current);
      if (!startRef.current) return;
      if (Math.abs((scrollNow?.scrollTop ?? 0) - startRef.current.scrollTop) > 2) {
        return;
      }
      armedRef.current = true;
      setArmed(true);
    }, 140);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLInputElement>) => {
    if (armedRef.current || !startRef.current) return;
    const scroll = scrollParentOf(e.currentTarget);
    if (Math.abs((scroll?.scrollTop ?? 0) - startRef.current.scrollTop) > 2) {
      clearArmTimer();
      return;
    }
    const dx = Math.abs(e.clientX - startRef.current.x);
    const dy = Math.abs(e.clientY - startRef.current.y);
    // Movement before arming = scroll / swipe intent, not a deliberate adjust.
    if (dx > 8 || dy > 8) clearArmTimer();
  };

  return (
    <input
      {...rest}
      ref={inputRef}
      type="range"
      className={className}
      value={value}
      data-scroll-safe={armed ? "armed" : "idle"}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={disarm}
      onPointerCancel={disarm}
      onChange={(e) => {
        if (!armedRef.current) return;
        onValueChange(Number(e.target.value));
      }}
    />
  );
}

interface MobileSettingsProps {
  /** Render in the desktop middle pane instead of a slide-over sheet. */
  embedded?: boolean;
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
  onExportLyrics: () => Promise<string | null>;
  onImportLyrics: () => Promise<string | null>;
  autoLyricsDownload: boolean;
  onAutoLyricsDownloadChange: (enabled: boolean) => void;
  eqSettings: EqSettings;
  onEqEnabledChange: (enabled: boolean) => void;
  onEqBandChange: (index: number, gain: number) => void;
  onEqPreset: (id: string) => void;
  onEqReset: () => void;
  crossfadeDuration: number;
  onCrossfadeChange: (value: number) => void;
  gaplessEnabled: boolean;
  onGaplessChange: (enabled: boolean) => void;
  currentOutputDevice: string;
  onSelectOutputDevice: (name: string) => Promise<void>;
  onResetApp: () => Promise<void>;
}

export default function MobileSettings({
  embedded = false,
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
  onExportLyrics,
  onImportLyrics,
  autoLyricsDownload,
  onAutoLyricsDownloadChange,
  eqSettings,
  onEqEnabledChange,
  onEqBandChange,
  onEqPreset,
  onEqReset,
  crossfadeDuration,
  onCrossfadeChange,
  gaplessEnabled,
  onGaplessChange,
  currentOutputDevice,
  onSelectOutputDevice,
  onResetApp,
}: MobileSettingsProps) {
  const [mediaFolders, setMediaFolders] = useState<string[]>([]);
  const [addingSource, setAddingSource] = useState(false);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [showAudioOutput, setShowAudioOutput] = useState(false);
  const [entered, setEntered] = useState(false);
  const [resetConfirming, setResetConfirming] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [listenStats, setListenStats] = useState<ListeningStats | null>(null);
  const [lyricsBusy, setLyricsBusy] = useState(false);
  const [lyricsStatus, setLyricsStatus] = useState<string | null>(null);

  const refreshMediaFolders = () => {
    listMediaFolders().then(setMediaFolders).catch(() => {});
  };

  const refreshListenStats = () => {
    getListeningStats(5)
      .then(setListenStats)
      .catch(() => setListenStats(null));
  };

  useEffect(() => {
    refreshMediaFolders();
    refreshListenStats();
    void (async () => {
      // Android only exposes ExoPlayer (system default) — hide the section.
      if (await isAndroid()) return;
      setShowAudioOutput(true);
      listOutputDevices().then(setOutputDevices).catch(() => {});
    })();
  }, []);

  useEffect(() => {
    const id = window.setInterval(refreshListenStats, 8000);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    if (embedded) {
      setEntered(true);
      return;
    }
    const id = requestAnimationFrame(() => {
      requestAnimationFrame(() => setEntered(true));
    });
    return () => cancelAnimationFrame(id);
  }, [embedded]);

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

  const handleExportLyricsClick = async () => {
    setLyricsBusy(true);
    setLyricsStatus(null);
    try {
      const message = await onExportLyrics();
      if (message) setLyricsStatus(message);
    } finally {
      setLyricsBusy(false);
    }
  };

  const handleImportLyricsClick = async () => {
    setLyricsBusy(true);
    setLyricsStatus(null);
    try {
      const message = await onImportLyrics();
      if (message) setLyricsStatus(message);
    } finally {
      setLyricsBusy(false);
    }
  };

  const orderedPlaylists = [...playlists].sort((a, b) => {
    const priority = [LIBRARY_PLAYLIST_NAME, "Favorites"];
    const ai = priority.indexOf(a.name);
    const bi = priority.indexOf(b.name);
    if (ai !== -1 || bi !== -1) return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    return a.name.localeCompare(b.name);
  });

  const rootClass = [
    embedded ? "main-content" : "",
    "mobile-settings-page",
    embedded ? "mset-embedded" : "",
    embedded || (entered && !closing) ? "mset-open" : "",
    !embedded && closing ? "mset-closing" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const Root = embedded ? "main" : "div";

  return (
    <Root className={rootClass}>
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
        {!embedded && (
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
        )}

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
          <h2>Lyrics</h2>
          <div className="mset-card">
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={autoLyricsDownload}
                onChange={(event) =>
                  onAutoLyricsDownloadChange(event.target.checked)
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Auto lyric download</span>
                <span className="mset-gapless-hint">
                  Fetch missing lyrics online when a song starts playing.
                </span>
              </span>
            </label>
            <div className="mset-playback-divider" role="separator" />
            <div className="mset-row">
              <div className="mset-row-text">
                <span className="mset-row-label">Saved lyrics</span>
                <span className="mset-row-value">
                  Back up downloaded lyrics, or restore them into matching
                  library tracks.
                </span>
              </div>
              <div className="mset-row-actions">
                <button
                  className="mset-icon-btn"
                  onClick={() => void handleImportLyricsClick()}
                  disabled={lyricsBusy}
                  type="button"
                  title="Import lyrics"
                >
                  <BiImport />
                </button>
                <button
                  className="mset-icon-btn"
                  onClick={() => void handleExportLyricsClick()}
                  disabled={lyricsBusy}
                  type="button"
                  title="Export lyrics"
                >
                  <BiExport />
                </button>
              </div>
            </div>
            {lyricsStatus && <p className="mset-hint mset-lyrics-status">{lyricsStatus}</p>}
          </div>
        </section>

        <section className="mset-section">
          <h2>Listening</h2>
          <div className="mset-card">
            {!listenStats ||
            (listenStats.tracks_played <= 0 &&
              listenStats.total_listen_seconds <= 0) ? (
              <p className="mset-hint">
                Play some music and Wave will summarize your listening here.
              </p>
            ) : (
              <>
                <div className="mset-listen-totals">
                  <div className="mset-listen-stat">
                    <span className="mset-listen-stat-value">
                      {formatListenDuration(listenStats.total_listen_seconds)}
                    </span>
                    <span className="mset-listen-stat-label">Total time</span>
                  </div>
                  <div className="mset-listen-stat">
                    <span className="mset-listen-stat-value">
                      {listenStats.total_plays.toLocaleString()}
                    </span>
                    <span className="mset-listen-stat-label">Plays</span>
                  </div>
                  <div className="mset-listen-stat">
                    <span className="mset-listen-stat-value">
                      {listenStats.tracks_played.toLocaleString()}
                    </span>
                    <span className="mset-listen-stat-label">Tracks</span>
                  </div>
                </div>

                {listenStats.top_tracks.length > 0 && (
                  <div className="mset-listen-group">
                    <h3 className="mset-listen-group-title">Top songs</h3>
                    <ul className="mset-listen-rank">
                      {listenStats.top_tracks.map((track, index) => (
                        <li
                          key={track.path}
                          className="mset-listen-rank-row"
                        >
                          <span className="mset-listen-rank-index">
                            {index + 1}
                          </span>
                          <span
                            className="mset-listen-rank-name"
                            title={getTrackTitle(track)}
                          >
                            {getTrackTitle(track)}
                            {track.artist ? (
                              <span className="mset-listen-rank-sub">
                                {" "}
                                · {track.artist}
                              </span>
                            ) : null}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}

                <ListenRankList
                  title="Top artists"
                  items={listenStats.top_artists}
                />
                <ListenRankList
                  title="Top albums"
                  items={listenStats.top_albums}
                />
                <ListenRankList
                  title="Top genres"
                  items={listenStats.top_genres}
                />
              </>
            )}
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
                  <ScrollSafeRange
                    min={-12}
                    max={12}
                    step={0.5}
                    value={eqSettings.bands[index] ?? 0}
                    onValueChange={(next) => onEqBandChange(index, next)}
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
          <div className="mset-card mset-playback-card">
            <div className="mset-crossfade-block">
              <div className="mset-crossfade-row">
                <ScrollSafeRange
                  min={0}
                  max={8}
                  step={0.5}
                  value={crossfadeDuration}
                  onValueChange={onCrossfadeChange}
                  aria-label="Crossfade duration in seconds"
                />
                <span className="mset-crossfade-value">
                  {crossfadeDuration === 0 ? "Off" : `${crossfadeDuration.toFixed(1)}s`}
                </span>
              </div>
              <p className="mset-hint mset-crossfade-hint">
                Smoothly blend the end of one track into the start of the next.
              </p>
            </div>
            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={gaplessEnabled}
                onChange={(event) => onGaplessChange(event.target.checked)}
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Gapless playback</span>
                <span className="mset-gapless-hint">
                  Play consecutive tracks without silence between them.
                </span>
              </span>
            </label>
          </div>
        </section>

        {showAudioOutput && (
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
        )}

        <section className="mset-section mset-danger-zone">
          <h2>
            <BiErrorCircle /> Reset
          </h2>
          <div className="mset-card mset-danger-card">
            <p className="mset-hint">
              Clear Wave&apos;s database and cached covers. Your audio files on
              disk stay put. Library and Favorites are kept empty; custom
              playlists and media folder links are removed.
            </p>
            {!resetConfirming ? (
              <button
                className="btn-ghost mset-danger-btn"
                type="button"
                onClick={() => setResetConfirming(true)}
              >
                <BiReset /> Clear database…
              </button>
            ) : (
              <div className="mset-danger-confirm">
                <p className="mset-danger-warning">
                  This cannot be undone. Continue?
                </p>
                <div className="mset-danger-actions">
                  <button
                    className="btn-ghost btn-sm"
                    type="button"
                    disabled={resetting}
                    onClick={() => setResetConfirming(false)}
                  >
                    Cancel
                  </button>
                  <button
                    className="btn-primary btn-sm mset-danger-confirm-btn"
                    type="button"
                    disabled={resetting}
                    onClick={() => {
                      void (async () => {
                        setResetting(true);
                        try {
                          await onResetApp();
                          setResetConfirming(false);
                          refreshMediaFolders();
                        } finally {
                          setResetting(false);
                        }
                      })();
                    }}
                  >
                    {resetting ? "Resetting…" : "Yes, reset Wave"}
                  </button>
                </div>
              </div>
            )}
          </div>
        </section>
      </div>
    </Root>
  );
}
