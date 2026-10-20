/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// Settings page: media source folders, playlist management, equalizer,
// crossfade, audio output (desktop/non-Android), and app reset.
import {
  useEffect,
  useId,
  useRef,
  useState,
  type InputHTMLAttributes,
  type ReactNode,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  BiArrowBack,
  BiCloud,
  BiFolder,
  BiHeadphone,
  BiListUl,
  BiMicrophone,
} from "react-icons/bi";
import type { IconType } from "react-icons";
import type { SourceSettings } from "../utils/player";
import {
  BiFolderOpen,
  BiFolderMinus,
  BiFolderPlus,
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
  BiVolumeFull,
  BiPalette,
  BiLink,
  BiAdjust,
  BiSun,
  BiMoon,
  BiDownload,
  BiLogoGithub,
} from "react-icons/bi";
import {
  listMediaFolders,
  listOutputDevices,
  getFileName,
  getListeningStats,
  formatListenDuration,
  EQ_BAND_LABELS,
  EQ_PRESETS,
  openRepository,
} from "../utils/player";
import type {
  EqSettings,
  ListeningStats,
  PlaylistInfo,
  ThemeMode,
} from "../utils/player";
import { isAndroid } from "../utils/platform";
import { formatBytes } from "../utils/format";
import { clearSourceCache, getSourceCacheUsage } from "../utils/remote";
import AddSourceChooser from "./AddSourceChooser";
import RemoteSourceList from "./RemoteSourceList";
import Select from "./Select";
import RemoteSourceSetupDialog from "./dialogs/RemoteSourceSetupDialog";

const CACHE_SIZES = [
  { mb: 512, label: "512 MB" },
  { mb: 1024, label: "1 GB" },
  { mb: 2048, label: "2 GB" },
  { mb: 5120, label: "5 GB" },
  { mb: 10240, label: "10 GB" },
];
import type { AppUpdate } from "../hooks/useAppUpdate";

const THEME_CHOICES: { mode: ThemeMode; label: string; Icon: IconType }[] = [
  { mode: "system", label: "System", Icon: BiAdjust },
  { mode: "light", label: "Light", Icon: BiSun },
  { mode: "dark", label: "Dark", Icon: BiMoon },
];

const formatMegabytes = (bytes: number) =>
  `${(bytes / (1024 * 1024)).toFixed(1)} MB`;

/** The line under the auto update switch: where updating stands. */
function UpdateStatusRow({ update }: { update: AppUpdate }) {
  const latest = update.latestVersion;
  let label: string;
  let hint: string | null = update.problem;
  let action: ReactNode = null;

  switch (update.status) {
    case "checking":
      label = "Checking for updates…";
      break;
    case "current":
      label = "Wave is up to date";
      break;
    case "available":
      label = `Wave ${latest} is available`;
      if (update.canDownload) {
        hint ??=
          update.downloadSize !== null
            ? `${formatMegabytes(update.downloadSize)} download`
            : null;
        action = (
          <button
            className="btn-primary btn-sm"
            type="button"
            onClick={update.download}
          >
            Download
          </button>
        );
      } else {
        hint ??= "Get it from the release page.";
        action = (
          <button
            className="btn-primary btn-sm"
            type="button"
            onClick={update.openPage}
          >
            Release page
          </button>
        );
      }
      break;
    case "downloading":
      label = `Downloading Wave ${latest}`;
      hint =
        update.progress !== null
          ? `${Math.round(update.progress * 100)}%`
          : null;
      break;
    case "ready":
      label = `Wave ${latest} is ready to install`;
      hint = "Android asks you to confirm the install.";
      action = (
        <button
          className="btn-primary btn-sm"
          type="button"
          onClick={update.install}
        >
          Install
        </button>
      );
      break;
    case "error":
      label = "Could not check for updates";
      break;
    default:
      label = "Not checked yet";
  }

  const canCheck =
    !update.autoUpdate &&
    (update.status === "idle" ||
      update.status === "current" ||
      update.status === "error");
  if (canCheck) {
    action = (
      <button
        className="btn-ghost btn-sm"
        type="button"
        onClick={update.checkNow}
      >
        Check for updates
      </button>
    );
  }

  return (
    <div className="mset-update-row" aria-live="polite">
      <span className="mset-gapless-copy">
        <span className="mset-gapless-label">{label}</span>
        {hint && <span className="mset-gapless-hint">{hint}</span>}
        {update.status === "downloading" && (
          <span className="mset-update-progress" aria-hidden>
            <span style={{ width: `${(update.progress ?? 0) * 100}%` }} />
          </span>
        )}
      </span>
      {action}
    </div>
  );
}

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
}: Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "onChange" | "type" | "value"
> & {
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
      if (
        Math.abs((scrollNow?.scrollTop ?? 0) - startRef.current.scrollTop) > 2
      ) {
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
  /** Add another scanned folder without changing the Library sync folder. */
  onAddExtraMediaSource: () => Promise<void>;
  /** Forget a saved media source; clears Library sync if it was that folder. */
  onRemoveMediaSource: (path: string) => Promise<void>;
  onExportLyrics: () => Promise<string | null>;
  onImportLyrics: () => Promise<string | null>;
  autoLyricsDownload: boolean;
  onAutoLyricsDownloadChange: (enabled: boolean) => void;
  waveformSeekEnabled: boolean;
  onWaveformSeekChange: (enabled: boolean) => void;
  themeMode: ThemeMode;
  onThemeModeChange: (mode: ThemeMode) => void;
  glassEffectsEnabled: boolean;
  onGlassEffectsChange: (enabled: boolean) => void;
  animationsEnabled: boolean;
  onAnimationsChange: (enabled: boolean) => void;
  reduceMotionEnabled: boolean;
  onReduceMotionChange: (enabled: boolean) => void;
  eqSettings: EqSettings;
  onEqEnabledChange: (enabled: boolean) => void;
  onEqBandChange: (index: number, gain: number) => void;
  onEqPreset: (id: string) => void;
  onEqReset: () => void;
  crossfadeDuration: number;
  onCrossfadeChange: (value: number) => void;
  gaplessEnabled: boolean;
  onGaplessChange: (enabled: boolean) => void;
  volumeNormalizationEnabled: boolean;
  onVolumeNormalizationChange: (enabled: boolean) => void;
  songCouplingEnabled: boolean;
  onSongCouplingChange: (enabled: boolean) => void;
  playlistMashingEnabled: boolean;
  onPlaylistMashingChange: (enabled: boolean) => void;
  sourceSettings: SourceSettings;
  onSourceSettingsChange: (settings: SourceSettings) => void;
  currentOutputDevice: string;
  onSelectOutputDevice: (name: string) => Promise<void>;
  onResetApp: () => Promise<void>;
  update: AppUpdate;
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
  onAddExtraMediaSource,
  onRemoveMediaSource,
  onExportLyrics,
  onImportLyrics,
  autoLyricsDownload,
  onAutoLyricsDownloadChange,
  waveformSeekEnabled,
  onWaveformSeekChange,
  themeMode,
  onThemeModeChange,
  glassEffectsEnabled,
  onGlassEffectsChange,
  animationsEnabled,
  onAnimationsChange,
  reduceMotionEnabled,
  onReduceMotionChange,
  eqSettings,
  onEqEnabledChange,
  onEqBandChange,
  onEqPreset,
  onEqReset,
  crossfadeDuration,
  onCrossfadeChange,
  gaplessEnabled,
  onGaplessChange,
  volumeNormalizationEnabled,
  onVolumeNormalizationChange,
  songCouplingEnabled,
  onSongCouplingChange,
  playlistMashingEnabled,
  onPlaylistMashingChange,
  sourceSettings,
  onSourceSettingsChange,
  currentOutputDevice,
  onSelectOutputDevice,
  onResetApp,
  update,
}: MobileSettingsProps) {
  // Both the embedded desktop pane and the mobile sheet can be mounted at
  // once, so the credential fields need ids unique to this instance.
  const keyIdPrefix = useId();
  const [mediaFolders, setMediaFolders] = useState<string[]>([]);
  const [addingSource, setAddingSource] = useState(false);
  const [choosingSource, setChoosingSource] = useState(false);
  const [settingUpRemote, setSettingUpRemote] = useState(false);
  const [cacheUsage, setCacheUsage] = useState<number | null>(null);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [showAudioOutput, setShowAudioOutput] = useState(false);
  const [entered, setEntered] = useState(false);
  const [resetConfirming, setResetConfirming] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [listenStats, setListenStats] = useState<ListeningStats | null>(null);
  const [lyricsBusy, setLyricsBusy] = useState(false);
  const [lyricsStatus, setLyricsStatus] = useState<string | null>(null);
  const [onAndroid, setOnAndroid] = useState(false);

  const refreshMediaFolders = () => {
    listMediaFolders()
      .then(setMediaFolders)
      .catch(() => {});
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
      if (await isAndroid()) {
        setOnAndroid(true);
        return;
      }
      setShowAudioOutput(true);
      listOutputDevices()
        .then(setOutputDevices)
        .catch(() => {});
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

  const refreshCacheUsage = () => {
    getSourceCacheUsage()
      .then(setCacheUsage)
      .catch(() => setCacheUsage(null));
  };

  const cacheShown =
    sourceSettings.outside_sourcing_enabled ||
    sourceSettings.remote_sources_enabled;
  useEffect(() => {
    if (cacheShown) refreshCacheUsage();
  }, [cacheShown, sourceSettings.cache_limit_mb]);

  const handleAddExtraSource = async () => {
    setAddingSource(true);
    try {
      await onAddExtraMediaSource();
    } finally {
      setAddingSource(false);
      refreshMediaFolders();
    }
  };

  const handleRemoveFolder = async (path: string) => {
    try {
      await onRemoveMediaSource(path);
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
    if (ai !== -1 || bi !== -1)
      return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
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
            <div className="mset-section-header">
              <h2>
                <BiFolder /> Media Source Folders
              </h2>
              <div className="mset-section-header-actions">
                <button
                  className="mset-icon-btn"
                  onClick={() => {
                    if (sourceSettings.remote_sources_enabled) {
                      setChoosingSource(true);
                    } else {
                      void handleAddExtraSource();
                    }
                  }}
                  disabled={addingSource || isScanningFolder}
                  type="button"
                  title="Add media source"
                >
                  <BiFolderPlus />
                </button>
              </div>
            </div>
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
                  Folders you've added as sources. Removing one just forgets it
                  — your music stays where it is.
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
            {sourceSettings.remote_sources_enabled && <RemoteSourceList />}
          </section>
        )}

        <section className="mset-section">
          <div className="mset-section-header">
            <h2>
              <BiListUl /> Playlists
            </h2>
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
              const locked =
                isLibraryPlaylistName(pl.name) || pl.name === "Favorites";
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
          <h2>
            <BiMicrophone /> Lyrics
          </h2>
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
            {lyricsStatus && (
              <p className="mset-hint mset-lyrics-status">{lyricsStatus}</p>
            )}
          </div>
        </section>

        <section className="mset-section">
          <h2>
            <BiCloud /> Music sources
          </h2>
          <div className="mset-card">
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={sourceSettings.outside_sourcing_enabled}
                onChange={(event) =>
                  onSourceSettingsChange({
                    ...sourceSettings,
                    outside_sourcing_enabled: event.target.checked,
                  })
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">
                  Search outside sources
                </span>
                <span className="mset-gapless-hint">
                  Look online when your library comes up empty. Nothing is
                  contacted until you ask for it.
                </span>
              </span>
            </label>

            {sourceSettings.outside_sourcing_enabled && (
              <>
                <div className="mset-playback-divider" role="separator" />
                <div className="mset-key">
                  <label
                    className="mset-key-label"
                    htmlFor={`${keyIdPrefix}src-jamendo`}
                  >
                    Jamendo
                  </label>
                  <input
                    id={`${keyIdPrefix}src-jamendo`}
                    type="text"
                    className="mset-key-input"
                    value={sourceSettings.jamendo_client_id}
                    placeholder="Client ID"
                    spellCheck={false}
                    autoCapitalize="none"
                    autoCorrect="off"
                    onChange={(event) =>
                      onSourceSettingsChange({
                        ...sourceSettings,
                        jamendo_client_id: event.target.value,
                      })
                    }
                  />
                </div>
                <div className="mset-key">
                  <label
                    className="mset-key-label"
                    htmlFor={`${keyIdPrefix}src-spotify`}
                  >
                    Spotify
                  </label>
                  <input
                    id={`${keyIdPrefix}src-spotify`}
                    type="text"
                    className="mset-key-input"
                    value={sourceSettings.spotify_client_id}
                    placeholder="Client ID"
                    spellCheck={false}
                    autoCapitalize="none"
                    autoCorrect="off"
                    onChange={(event) =>
                      onSourceSettingsChange({
                        ...sourceSettings,
                        spotify_client_id: event.target.value,
                      })
                    }
                  />
                </div>
                <p className="mset-hint mset-key-note">
                  Deezer and Internet Archive need no key. Deezer returns
                  30-second clips; Spotify serves no audio at all.
                </p>
              </>
            )}

            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={sourceSettings.remote_sources_enabled}
                onChange={(event) =>
                  onSourceSettingsChange({
                    ...sourceSettings,
                    remote_sources_enabled: event.target.checked,
                  })
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Outside Sourcing</span>
                <span className="mset-gapless-hint">
                  Play music from your own Plex, Jellyfin or Navidrome server.
                  Wave won't connect to anything while this is off.
                </span>
              </span>
            </label>

            {sourceSettings.remote_sources_enabled && onAndroid && (
              <label className="mset-gapless-row mset-nested-row">
                <input
                  type="checkbox"
                  checked={sourceSettings.remote_cellular_enabled}
                  onChange={(event) =>
                    onSourceSettingsChange({
                      ...sourceSettings,
                      remote_cellular_enabled: event.target.checked,
                    })
                  }
                />
                <span className="mset-gapless-copy">
                  <span className="mset-gapless-label">Use cellular data</span>
                  <span className="mset-gapless-hint">
                    When off, music from your servers only plays on Wi-Fi.
                  </span>
                </span>
              </label>
            )}

            {/* Desktop has no media source folders, so remote sources are
                added here as well as from the Add menu. */}
            {sourceSettings.remote_sources_enabled && embedded && (
              <>
                <div className="mset-playback-divider" role="separator" />
                <div className="mset-row">
                  <span className="mset-row-text">
                    <span className="mset-row-label">Remote sources</span>
                    <span className="mset-row-value">
                      Plex, Jellyfin, Emby and Subsonic servers
                    </span>
                  </span>
                  <button
                    className="btn-secondary mset-row-btn"
                    type="button"
                    onClick={() => setSettingUpRemote(true)}
                  >
                    Add remote source
                  </button>
                </div>
                <RemoteSourceList />
              </>
            )}

            {(sourceSettings.outside_sourcing_enabled ||
              sourceSettings.remote_sources_enabled) && (
              <>
                <div className="mset-playback-divider" role="separator" />
                <div className="mset-row mset-cache-row">
                  <span className="mset-row-text">
                    <label
                      className="mset-row-label"
                      htmlFor={`${keyIdPrefix}cache-size`}
                    >
                      Cache size
                    </label>
                    <span className="mset-row-value">
                      Music saved for offline from your servers and outside
                      sources
                    </span>
                  </span>
                  <Select
                    id={`${keyIdPrefix}cache-size`}
                    value={String(sourceSettings.cache_limit_mb)}
                    options={[
                      ...(CACHE_SIZES.some(
                        (size) => size.mb === sourceSettings.cache_limit_mb,
                      )
                        ? []
                        : [
                            {
                              value: String(sourceSettings.cache_limit_mb),
                              label: formatBytes(
                                sourceSettings.cache_limit_mb * 1024 * 1024,
                              ),
                            },
                          ]),
                      ...CACHE_SIZES.map((size) => ({
                        value: String(size.mb),
                        label: size.label,
                      })),
                    ]}
                    onChange={(value) =>
                      onSourceSettingsChange({
                        ...sourceSettings,
                        cache_limit_mb: Number(value),
                      })
                    }
                  />
                </div>
                <div className="mset-row mset-cache-row">
                  <span className="mset-row-text">
                    <span className="mset-row-label">Cached music</span>
                    <span className="mset-row-value">
                      {cacheUsage === null
                        ? "Checking…"
                        : `${formatBytes(cacheUsage)} used`}
                    </span>
                  </span>
                  <button
                    className="btn-secondary mset-row-btn"
                    type="button"
                    onClick={() => {
                      void clearSourceCache()
                        .catch(() => 0)
                        .then(refreshCacheUsage);
                    }}
                  >
                    Clear
                  </button>
                </div>
              </>
            )}
          </div>
        </section>

        {choosingSource && (
          <AddSourceChooser
            onCancel={() => setChoosingSource(false)}
            onLocalFolder={() => {
              setChoosingSource(false);
              void handleAddExtraSource();
            }}
            onRemoteSource={() => {
              setChoosingSource(false);
              setSettingUpRemote(true);
            }}
          />
        )}

        {settingUpRemote && (
          <RemoteSourceSetupDialog onClose={() => setSettingUpRemote(false)} />
        )}

        <section className="mset-section">
          <h2>
            <BiHeadphone /> Listening
          </h2>
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
                        <li key={track.path} className="mset-listen-rank-row">
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
            <Select
              className="eq-preset-select mset-eq-preset"
              value=""
              placeholder="Presets"
              options={EQ_PRESETS.map((preset) => ({
                value: preset.id,
                label: preset.label,
              }))}
              onChange={onEqPreset}
              aria-label="EQ preset"
            />
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
            <button
              className="btn-ghost btn-sm"
              onClick={onEqReset}
              type="button"
            >
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
                  {crossfadeDuration === 0
                    ? "Off"
                    : `${crossfadeDuration.toFixed(1)}s`}
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

        <section className="mset-section">
          <h2>
            <BiVolumeFull /> Volume normalization
          </h2>
          <div className="mset-card mset-playback-card">
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={volumeNormalizationEnabled}
                onChange={(event) =>
                  onVolumeNormalizationChange(event.target.checked)
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Normalize volume</span>
                <span className="mset-gapless-hint">
                  Even out volume across your queue by boosting quiet tracks and
                  turning down loud ones toward the median loudness, without
                  clipping.
                </span>
              </span>
            </label>
          </div>
        </section>

        <section className="mset-section">
          <h2>
            <BiLink /> Coupling and mashing
          </h2>
          <div className="mset-card mset-playback-card">
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={songCouplingEnabled}
                onChange={(event) => onSongCouplingChange(event.target.checked)}
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Couple songs</span>
                <span className="mset-gapless-hint">
                  Link two songs so they always play back to back, in the order
                  you pick, with shuffle on or off. Couple songs from any song's
                  menu.
                </span>
              </span>
            </label>
            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={playlistMashingEnabled}
                onChange={(event) =>
                  onPlaylistMashingChange(event.target.checked)
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Mash playlists</span>
                <span className="mset-gapless-hint">
                  Mix albums and playlists into one shuffled queue. Start a
                  mashup from the link button on any album or playlist, and
                  press the link in the player to shuffle it again.
                </span>
              </span>
            </label>
          </div>
        </section>

        <section className="mset-section">
          <h2>
            <BiPalette /> Appearance
          </h2>
          <div className="mset-card mset-playback-card">
            <div className="mset-theme-row">
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Theme</span>
                <span className="mset-gapless-hint">
                  System follows your device's light or dark setting and
                  switches along with it.
                </span>
              </span>
              <div
                className="mset-theme-choice"
                role="radiogroup"
                aria-label="Theme"
              >
                {THEME_CHOICES.map(({ mode, label, Icon }) => (
                  <button
                    key={mode}
                    type="button"
                    role="radio"
                    aria-checked={themeMode === mode}
                    className={themeMode === mode ? "active" : ""}
                    onClick={() => onThemeModeChange(mode)}
                  >
                    <Icon aria-hidden />
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={waveformSeekEnabled}
                onChange={(event) => onWaveformSeekChange(event.target.checked)}
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Waveform seek bar</span>
                <span className="mset-gapless-hint">
                  Show each track's waveform in the seek bar, moving with the
                  music around the playhead. Turn off for a plain slider and no
                  track analysis.
                </span>
              </span>
            </label>
            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={glassEffectsEnabled}
                onChange={(event) => onGlassEffectsChange(event.target.checked)}
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Glass effect</span>
                <span className="mset-gapless-hint">
                  Blur what sits behind bars, panels and sheets. Turn off for
                  solid backgrounds, which are lighter on slower phones.
                </span>
              </span>
            </label>
            <div className="mset-playback-divider" role="separator" />
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={animationsEnabled}
                onChange={(event) => onAnimationsChange(event.target.checked)}
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Animations</span>
                <span className="mset-gapless-hint">
                  Slide pages in, fade menus and scroll lyrics smoothly. Turn
                  off to make every change instant.
                </span>
              </span>
            </label>
            {animationsEnabled && (
              <>
                <div className="mset-playback-divider" role="separator" />
                <label className="mset-gapless-row">
                  <input
                    type="checkbox"
                    checked={reduceMotionEnabled}
                    onChange={(event) =>
                      onReduceMotionChange(event.target.checked)
                    }
                  />
                  <span className="mset-gapless-copy">
                    <span className="mset-gapless-label">Reduce motion</span>
                    <span className="mset-gapless-hint">
                      Fade pages and sheets in place instead of sliding them
                      across the screen, and move lyrics from line to line
                      without scrolling or sweeping.
                    </span>
                  </span>
                </label>
              </>
            )}
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

        <section className="mset-section">
          <h2>
            <BiDownload /> Updates
          </h2>
          <div className="mset-card mset-playback-card">
            <label className="mset-gapless-row">
              <input
                type="checkbox"
                checked={update.autoUpdate}
                onChange={(event) =>
                  void update.handleAutoUpdateChange(event.target.checked)
                }
              />
              <span className="mset-gapless-copy">
                <span className="mset-gapless-label">Auto update</span>
                <span className="mset-gapless-hint">
                  {onAndroid
                    ? "Look for a new version when Wave starts and download it in the background. Android asks you to confirm before it installs."
                    : "Look for a new version when Wave starts and let you know when one is out."}
                </span>
              </span>
            </label>
            {(!update.autoUpdate || update.status !== "idle") && (
              <>
                <div className="mset-playback-divider" role="separator" />
                <UpdateStatusRow update={update} />
              </>
            )}
          </div>
        </section>

        <section className="mset-section mset-danger-zone">
          <h2>
            <BiErrorCircle /> Reset
          </h2>
          <div className="mset-card">
            <div className="mset-row mset-danger-row">
              <span className="mset-row-text">
                <span className="mset-row-label">Clear database</span>
                <span className="mset-gapless-hint">
                  {resetConfirming
                    ? "This can't be undone. Your audio files stay; everything else in Wave is cleared."
                    : "Empties your library and favorites, and removes playlists, folder links and cached covers. Audio files on disk stay put."}
                </span>
              </span>
              {!resetConfirming ? (
                <button
                  className="mset-row-btn mset-danger-btn"
                  type="button"
                  onClick={() => setResetConfirming(true)}
                >
                  <BiReset /> Clear…
                </button>
              ) : (
                <span className="mset-row-actions">
                  <button
                    className="btn-ghost mset-row-btn"
                    type="button"
                    disabled={resetting}
                    onClick={() => setResetConfirming(false)}
                  >
                    Cancel
                  </button>
                  <button
                    className="mset-row-btn mset-danger-confirm-btn"
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
                    {resetting ? "Clearing…" : "Clear"}
                  </button>
                </span>
              )}
            </div>
          </div>
        </section>

        <footer className="mset-about">
          <p className="mset-about-credit">
            Built with <span aria-label="love">❤️</span> by Behdad
          </p>
          <p>
            Wave is free software, licensed under the GNU Affero General Public
            License v3.0 with additional terms. It comes with no warranty, to
            the extent permitted by law. The Wave name and logo are not covered
            by that license.
          </p>
          <p>Copyright © {new Date().getFullYear()} BMDarkLight</p>
          <p className="mset-about-version">
            {update.currentVersion && <>Version {update.currentVersion} · </>}
            <a
              href="https://github.com/BMDarkLight/Wave"
              onClick={(event) => {
                event.preventDefault();
                openRepository().catch(() => {});
              }}
            >
              <BiLogoGithub aria-hidden /> Source on GitHub
            </a>
          </p>
        </footer>
      </div>
    </Root>
  );
}
