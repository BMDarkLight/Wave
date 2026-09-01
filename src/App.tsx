// The Code for Frontend of Wave is currently completely AI Generated and may contain bugs or rough edges. Please report any issues you encounter at

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import trayTemplate from "../assets/tray-template.svg";
import { BiX, BiFolderOpen, BiMenu, BiSearch } from "react-icons/bi";
import type { SourceSettings, SourceTrack } from "./utils/player";
import {
  addTrackToPlaylistById,
  getSourceSettings,
  listenToDownloadFallback,
  setSourceSettings,
  clearAudioImports,
  createPlaylist,
  resetApp,
  exportLyrics,
  getTrackDetails,
  getFavorites,
  getPlaybackMode,
  getPlaybackState,
  getPlaylistTracksById,
  importLyrics,
  scanDirectory,
  listPlaylists,
  listenToMediaControls,
  openLyricsDialog,
  pauseTrack,
  playNext,
  playPrevious,
  playTrack,
  playTracks,
  playTrackFromSpecificPlaylist,
  renamePlaylist,
  resumeTrack,
  saveLyricsDialog,
  seekTrack,
  setPlayerVolume,
  setRepeat,
  setShuffle,
  updateMediaMetadata,
  listOutputDevices,
  setOutputDevice,
  scanDirectoryRecursive,
  syncPlaylistFolder,
  takeAndroidCrashReport,
  clearAndroidCrashReport,
  listenToSyncProgress,
  getRecentlyPlayed,
  getMostPlayed,
  type PlaylistInfo,
  type Track,
} from "./utils/player";
import { isAndroid } from "./utils/platform";
import { formatInvokeError } from "./utils/errors";
import { formatTime } from "./utils/format";
import {
  LIBRARY_PLAYLIST_NAME,
  isLibraryPlaylistName,
  getTrackTitle,
} from "./utils/track";
import {
  MATCH_FIELD_LABEL,
  hitMatchesSearchScope,
  mainSearchScopeLabel,
  highlightMatch,
  type MainSearchScope,
} from "./utils/search";
import { useEqualizerSettings } from "./hooks/useEqualizerSettings";
import { useResizablePanels } from "./hooks/useResizablePanels";
import { useLibrarySearch } from "./hooks/useLibrarySearch";
import { useSourceSearch } from "./hooks/useSourceSearch";
import { SourceResults } from "./components/SourceResults";
import { useLyricsPanel } from "./hooks/useLyricsPanel";
import { useMobileOverlays } from "./hooks/useMobileOverlays";
import { usePlaylistManager } from "./hooks/usePlaylistManager";
import { usePlaybackController } from "./hooks/usePlaybackController";
import { useMediaImport } from "./hooks/useMediaImport";
import { useAndroidBackTrap } from "./hooks/useAndroidBackTrap";
import { armDragDismissGhostClickGuard } from "./hooks/useDragDismiss";
import AlbumPage from "./components/AlbumPage";
import ArtistPage from "./components/ArtistPage";
import HomePage from "./components/HomePage";
import PlayedTracksPage from "./components/PlayedTracksPage";
import Artwork from "./components/Artwork";
import ClearPlaylistDialog from "./components/dialogs/ClearPlaylistDialog";
import DeletePlaylistDialog from "./components/dialogs/DeletePlaylistDialog";
import AddToPlaylistDialog from "./components/dialogs/AddToPlaylistDialog";
import AddFromLibraryDialog from "./components/dialogs/AddFromLibraryDialog";
import CreatePlaylistDialog from "./components/dialogs/CreatePlaylistDialog";
import TrackContextMenu from "./components/TrackContextMenu";
import QueueContextMenu from "./components/QueueContextMenu";
import AddTrackMenu from "./components/AddTrackMenu";
import StatusToasts from "./components/StatusToasts";
import EqPanel from "./components/EqPanel";
import QueuePanel from "./components/QueuePanel";
import LyricsPanel from "./components/LyricsPanel";
import DeviceListPanel from "./components/DeviceListPanel";
import PlayerBar from "./components/PlayerBar";
import Sidebar, { type MainView } from "./components/Sidebar";
import LibraryTrackList from "./components/LibraryTrackList";
import MobileNowPlaying from "./components/MobileNowPlaying";
import MobileSettings from "./components/MobileSettings";
import "./App.css";
import "./touch-hover.css";

function App() {
  const [playlist, setPlaylist] = useState<Track[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [sourceSettings, setSourceSettingsState] = useState<SourceSettings>({
    // Matches the backend default, so the tier stays hidden until settings load.
    outside_sourcing_enabled: false,
    jamendo_client_id: "",
    spotify_client_id: "",
  });
  const [crashReport, setCrashReport] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingPlaylist, setIsLoadingPlaylist] = useState(true);
  // Playlist management
  const [playlists, setPlaylists] = useState<PlaylistInfo[]>([]);
  const [selectedPlaylistId, setSelectedPlaylistId] = useState<string | null>(
    null,
  );
  /** Top-level main pane: Home suggestions vs playlist vs listen stats / settings. */
  const [mainView, setMainView] = useState<MainView>("home");

  // Favorited track paths (for heart toggle state in the track list)
  const [favoritePaths, setFavoritePaths] = useState<Set<string>>(new Set());

  // Refresh the set of favorited track paths (drives heart toggle state).
  const loadFavoritePaths = async () => {
    try {
      const favorites = await getFavorites();
      setFavoritePaths(new Set(favorites.map((t) => t.path)));
    } catch (err) {
      // Loading favorites is best-effort; don't surface hard errors for the heart UI.
      console.warn("Failed to load favorites:", err);
    }
  };

  const [mobileNavOpen, setMobileNavOpen] = useState(false);
  const [androidHost, setAndroidHost] = useState(false);
  const androidHostRef = useRef(false);
  // Create / rename playlist dialog
  const addTrackBtnRef = useRef<HTMLButtonElement>(null);
  const selectedPlaylistIdRef = useRef<string | null>(null);

  const setActivePlaylistId = (id: string | null) => {
    selectedPlaylistIdRef.current = id;
    setSelectedPlaylistId(id);
  };

  const {
    browseStack,
    viewingAlbum,
    viewingArtist,
    openArtistPage,
    openAlbumPage,
    pushAlbumPage,
    browseBack,
    clearBrowse,
    showClearConfirm,
    setShowClearConfirm,
    deletePlaylistConfirm,
    setDeletePlaylistConfirm,
    playlistDialog,
    playlistNameInput,
    setPlaylistNameInput,
    playlistSyncFolder,
    setPlaylistSyncFolderInput,
    playlistDialogError,
    setPlaylistDialogError,
    playlistNameInputRef,
    selectedPlaylist,
    libraryPlaylist,
    favoritesPlaylist,
    userPlaylists,
    loadPlaylists,
    loadPlaylistTracks,
    getDefaultPlaylistId,
    handleClearPlaylist,
    confirmClearPlaylist,
    openCreatePlaylistDialog,
    openRenamePlaylistDialog,
    closePlaylistDialog,
    pickPlaylistSyncFolder,
    handleDeletePlaylist,
    confirmDeletePlaylist,
    handleExportPlaylistById,
    handleImportPlaylist,
  } = usePlaylistManager({
    playlists,
    setPlaylists,
    selectedPlaylistId,
    androidHost,
    setError,
    setPlaylist,
    setIsLoadingPlaylist,
    setIsLoading,
    loadFavoritePaths,
    setMobileNavOpen,
    setMainView,
    setActivePlaylistId,
    selectedPlaylistIdRef,
  });
  const {
    mainSearchQuery,
    setMainSearchQuery,
    mainSearchHits,
    mainSearchLoading,
    mainSearchFullLibrary,
    setMainSearchFullLibrary,
    mainSearchOpen,
    mainSearchInputRef,
    mobileSearchInputRef,
    focusMainSearchInput,
    openMainSearch,
    closeMainSearch,
    toggleMainSearch,
  } = useLibrarySearch();

  // Tier 3 of the search ladder. Manual by design: unlike scope and library,
  // this one costs a network round trip per provider.
  const {
    sourceResults,
    sourceLoading,
    sourceSearched,
    sourceError,
    sourceBusy,
    searchSourcesNow,
    streamSourceHit,
    downloadSourceHit,
  } = useSourceSearch(mainSearchQuery);

  // Audio output device selection
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [showDeviceList, setShowDeviceList] = useState(false);

  // Equalizer
  const {
    showEqPanel,
    setShowEqPanel,
    eqSettings,
    crossfadeDuration,
    gaplessEnabled,
    volumeNormalizationEnabled,
    autoLyricsDownload,
    eqAnchor,
    setEqAnchor,
    volumeIconRef,
    loadEqSettings,
    handleToggleEqPanel,
    handleEqEnabled,
    handleEqBandChange,
    handleEqBandsChange,
    handleEqPreset,
    handleEqReset,
    handleCrossfadeChange,
    handleGaplessChange,
    handleVolumeNormalizationChange,
    handleAutoLyricsDownloadChange,
  } = useEqualizerSettings(setError);

  const {
    showFolderSetup,
    setShowFolderSetup,
    skipFolderSetup,
    mobilePlayerOpen,
    setMobilePlayerOpen,
    mobilePlayerClosing,
    setMobilePlayerClosing,
    mobilePlayerKey,
    setMobilePlayerKey,
    mobilePlayerOpenRef,
    mobilePlayerClosingRef,
    mobilePlayerCloseTimer,
    mobilePlayerView,
    setMobilePlayerView,
    mobilePlayerMenuOpen,
    setMobilePlayerMenuOpen,
    mobileSettingsOpen,
    setMobileSettingsOpen,
    mobileSettingsClosing,
    setMobileSettingsClosing,
    mobileSettingsClosingRef,
    mobileSettingsCloseTimer,
    forceCloseMobileSettings,
    forceCloseMobilePlayer,
    handleCloseMobilePlayer,
    handleDragCloseMobilePlayer,
  } = useMobileOverlays({
    mobileNavOpen,
    setMobileNavOpen,
    androidHost,
    setAndroidHost,
    androidHostRef,
    playlists,
    setMainView,
    clearBrowse,
    onAndroidDetected: () => {
      setVolumeValue(1);
      void setPlayerVolume(1);
    },
  });

  // Android uses system volume — keep Wave at 100% always.
  useEffect(() => {
    if (!androidHost) return;
    setVolumeValue(1);
    void setPlayerVolume(1);
  }, [androidHost]);

  // Opening search dismisses drawers/panels that would cover it.
  useEffect(() => {
    if (!mainSearchOpen) return;
    setMobileNavOpen(false);
    setShowQueue(false);
    setShowDeviceList(false);
    setLyricsPanelTrack(null);
    // Wait a beat so the mobile topbar expansion has started before focusing.
    const id = window.setTimeout(() => focusMainSearchInput(), 180);
    return () => window.clearTimeout(id);
  }, [mainSearchOpen]);

  // Track context menu
  const [menuTrackPath, setMenuTrackPath] = useState<string | null>(null);
  const [menuAnchor, setMenuAnchor] = useState<{
    top: number;
    right?: number;
    left?: number;
    flipAbove?: number;
  } | null>(null);
  const [addToPlaylistTrack, setAddToPlaylistTrack] = useState<string | null>(
    null,
  );

  // Sort state — cycles asc → desc → off on repeated header clicks
  const [sortColumn, setSortColumn] = useState<"index" | "title" | "album">(
    "index",
  );
  const [sortDirection, setSortDirection] = useState<"asc" | "desc" | "none">(
    "asc",
  );

  // Resizable title/album split. Album uses minmax(0, width) so it collapses
  // first when the main pane shrinks; title keeps a larger minimum.
  const [albumColWidth, setAlbumColWidth] = useState(200);
  const trackGridCols = `48px minmax(200px, 1fr) minmax(0px, ${albumColWidth}px) 64px 40px`;

  const handleAlbumColResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startWidth = albumColWidth;
    const onMouseMove = (ev: MouseEvent) => {
      // Handle sits on the title/album boundary: drag right → title grows, album shrinks.
      const dx = ev.clientX - startX;
      // Preferred album width from the drag; layout may still collapse it
      // further via minmax(0, …) when the window is narrow.
      setAlbumColWidth(Math.max(72, Math.min(480, startWidth - dx)));
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  };

  const sortedPlaylist = useMemo(() => {
    const sorted = [...playlist];
    if (sortDirection === "none") return sorted;
    if (sortColumn === "title") {
      sorted.sort((a, b) =>
        (getTrackTitle(a) ?? "").localeCompare(getTrackTitle(b) ?? ""),
      );
    } else if (sortColumn === "album") {
      sorted.sort((a, b) => (a.album ?? "").localeCompare(b.album ?? ""));
    }
    if (sortDirection === "desc") sorted.reverse();
    return sorted;
  }, [playlist, sortColumn, sortDirection]);

  const {
    playbackState,
    setSeekValue,
    volumeValue,
    setVolumeValue,
    playbackMode,
    queueData,
    setQueueData,
    showQueue,
    setShowQueue,
    queueMenuIndex,
    setQueueMenuIndex,
    queueMenuAnchor,
    setQueueMenuAnchor,
    currentTrack,
    canSkip,
    displayDuration,
    displayPosition,
    updatePlaybackState,
    loadPlaybackMode,
    loadQueueTracks,
    handlePlayTrack,
    handlePlayPause,
    handleStop,
    handlePrevious,
    handleNext,
    handleSeek,
    handleVolume,
    handleToggleFavorite,
    handlePlayNext,
    handleAddToQueue,
    handleRemoveFromQueue,
    handleMoveQueueTrack,
    closeQueueContextMenu,
    openQueueContextMenu,
    handleClearQueue,
    handleToggleShuffle,
    handleCycleRepeat,
    handlePlayFromQueue,
  } = usePlaybackController({
    androidHost,
    androidHostRef,
    playlist,
    playlists,
    sortedPlaylist,
    sortDirection,
    selectedPlaylistId,
    loadPlaylists,
    loadPlaylistTracks,
    favoritePaths,
    setFavoritePaths,
    setError,
    setMenuTrackPath,
    setAddToPlaylistTrack,
    onNoTrackFallback: () => {
      void handleAddTrack(false);
    },
  });

  const {
    isAddingTracks,
    importingPlaylistId,
    importingPlaylistIdRef,
    isImporting,
    importedCount,
    showAddTrackMenu,
    setShowAddTrackMenu,
    addTrackMenuAnchor,
    setAddTrackMenuAnchor,
    showAddFromLibrary,
    librarySearchQuery,
    setLibrarySearchQuery,
    librarySearchResults,
    librarySearchSelected,
    librarySearchLoading,
    librarySearchAdding,
    isScanningFolder,
    setIsScanningFolder,
    folderScanIsSync,
    setFolderScanIsSync,
    beginPlaylistImport,
    endPlaylistImport,
    handleAddTrack,
    openAddFromLibrary,
    closeAddFromLibrary,
    toggleLibrarySearchSelect,
    handleAddSelectedFromLibrary,
    handlePickFileFromLibraryModal,
    handleAddFolder,
    handleAddFolderAndroid,
    handleAddMediaSource,
    handleAddExtraMediaSource,
    handleRemoveMediaSource,
    handleSelectOutputDeviceSettings,
    handleAddFolderAsPlaylist,
    handleSyncPlaylistFolder,
    setImportedCount,
    handleRemoveFromLibrary,
    handleRemoveFromPlaylist,
  } = useMediaImport({
    androidHost,
    selectedPlaylistId,
    selectedPlaylistIdRef,
    selectedPlaylist,
    playlists,
    loadPlaylists,
    loadPlaylistTracks,
    getDefaultPlaylistId,
    setActivePlaylistId,
    setMainView,
    setError,
    setShowFolderSetup,
    playbackState,
    setSeekValue,
    loadQueueTracks,
    updatePlaybackState,
    loadFavoritePaths,
  });

  const handleSort = (column: typeof sortColumn) => {
    if (sortColumn === column) {
      setSortDirection((d) =>
        d === "asc" ? "desc" : d === "desc" ? "none" : "asc",
      );
    } else {
      setSortColumn(column);
      setSortDirection("asc");
    }
  };

  // Panel toggles (only one open at a time)
  const handleToggleQueue = () => {
    setMobileNavOpen(false);
    closeMainSearch();
    setRightPanelWidth((width) => clampRightPanelWidth(width));
    if (showQueue) {
      closeRightPanelDelayed();
      return;
    }
    cancelCloseRightPanel();
    setShowDeviceList(false);
    setLyricsPanelTrack(null);
    setShowQueue(true);
    void loadEqSettings();
  };

  const handleToggleLyrics = () => {
    setMobileNavOpen(false);
    closeMainSearch();
    setRightPanelWidth((width) => clampRightPanelWidth(width));
    if (lyricsPanelTrack) {
      closeRightPanelDelayed();
      return;
    }
    cancelCloseRightPanel();
    setShowQueue(false);
    setShowDeviceList(false);
    setLyricsPanelTrack(currentTrack ?? null);
  };

  const handleOpenLyrics = () => {
    if (!currentTrack) return;
    // Second click on art/title should close the sidebar, not replace the
    // enriched panel track with a stale playlist entry (which drops lyrics).
    if (lyricsPanelTrack?.path === currentTrack.path) {
      closeRightPanelDelayed();
      return;
    }
    setMobileNavOpen(false);
    closeMainSearch();
    setRightPanelWidth((width) => clampRightPanelWidth(width));
    cancelCloseRightPanel();
    setShowQueue(false);
    setShowDeviceList(false);
    setLyricsPanelTrack(currentTrack);
  };

  // ── Mobile-only Settings / Now Playing page open-close ──────────────────
  const handleOpenMobilePlayer = () => {
    if (!currentTrack) return;
    setMobileNavOpen(false);
    closeMainSearch();
    setShowQueue(false);
    setShowDeviceList(false);
    setLyricsPanelTrack(null);
    forceCloseMobileSettings();
    const wasOpen = mobilePlayerOpenRef.current;
    const wasClosing = mobilePlayerClosingRef.current;
    if (mobilePlayerCloseTimer.current) {
      clearTimeout(mobilePlayerCloseTimer.current);
      mobilePlayerCloseTimer.current = null;
    }
    mobilePlayerClosingRef.current = false;
    setMobilePlayerClosing(false);
    setMobilePlayerView("cover");
    setMobilePlayerMenuOpen(false);
    // Remount when opening from closed/closing so enter animation and
    // pointer-events reset cleanly (avoids an invisible-but-open sheet).
    if (!wasOpen || wasClosing) {
      setMobilePlayerKey((key) => key + 1);
    }
    mobilePlayerOpenRef.current = true;
    setMobilePlayerOpen(true);
    void loadEqSettings();
  };

  /** Album art / track name tap in the mini player bar: mobile gets the
   *  fullscreen Now Playing page, desktop keeps the lyrics sidebar. */
  const handleOpenNowPlaying = () => {
    if (isMobileLayout()) handleOpenMobilePlayer();
    else handleOpenLyrics();
  };

  const handleOpenMobileSettings = () => {
    setMobileNavOpen(false);
    closeMainSearch();
    setShowQueue(false);
    setShowDeviceList(false);
    setLyricsPanelTrack(null);
    forceCloseMobilePlayer();
    void loadEqSettings();
    if (isMobileLayout()) {
      if (mobileSettingsCloseTimer.current) {
        clearTimeout(mobileSettingsCloseTimer.current);
        mobileSettingsCloseTimer.current = null;
      }
      mobileSettingsClosingRef.current = false;
      setMobileSettingsClosing(false);
      setMobileSettingsOpen(true);
      return;
    }
    // Desktop: show Settings in the middle pane like Home / playlists.
    forceCloseMobileSettings();
    clearBrowse();
    setMainView("settings");
  };

  const handleCloseMobileSettings = () => {
    if (mainView === "settings" && !isMobileLayout()) {
      setMainView("home");
      return;
    }
    if (!mobileSettingsOpen || mobileSettingsClosingRef.current) return;
    mobileSettingsClosingRef.current = true;
    setMobileSettingsClosing(true);
    mobileSettingsCloseTimer.current = setTimeout(() => {
      mobileSettingsClosingRef.current = false;
      setMobileSettingsClosing(false);
      setMobileSettingsOpen(false);
      mobileSettingsCloseTimer.current = null;
    }, 320);
  };

  const handleResetApp = async () => {
    try {
      setError(null);
      await resetApp();
      // Fresh boot so every cached list / session bit is rebuilt from the empty DB.
      window.location.reload();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to reset Wave"));
      throw err;
    }
  };

  const handleToggleDevice = () => {
    setMobileNavOpen(false);
    setRightPanelWidth((width) => clampRightPanelWidth(width));
    if (showDeviceList) {
      closeRightPanelDelayed();
      return;
    }
    cancelCloseRightPanel();
    setShowQueue(false);
    setLyricsPanelTrack(null);
    setShowDeviceList(true);
  };

  /** Reconcile every playlist that has a sync_folder with its folder on disk. */
  const syncFolderPlaylists = async (
    list: PlaylistInfo[],
    isAndroidDevice: boolean,
  ) => {
    const synced = list.filter((p) => p.sync_folder);
    if (!synced.length) return;

    setIsScanningFolder(true);
    setFolderScanIsSync(true);
    setImportedCount(0);
    const stopProgress = await listenToSyncProgress((p) => {
      if (typeof p.processed === "number") setImportedCount(p.processed);
      else if (typeof p.extracted === "number") setImportedCount(p.extracted);
      else if (typeof p.added === "number") setImportedCount(p.added);
    }).catch(() => null);

    let failed = 0;
    for (const [i, pl] of synced.entries()) {
      const firstTimeFill = pl.track_count === 0;
      if (firstTimeFill) beginPlaylistImport(pl.id);
      try {
        const folder = pl.sync_folder!;
        const paths = isAndroidDevice
          ? await scanDirectoryRecursive(folder)
          : null; // desktop: Rust walks sync_folder itself
        await syncPlaylistFolder(pl.id, paths);
        // Soft-refresh the open playlist without clearing the list first —
        // but not while a first-time import UI is covering that playlist.
        const viewId = selectedPlaylistIdRef.current;
        if (!firstTimeFill && (viewId === pl.id || (!viewId && i === 0))) {
          const id = viewId ?? pl.id;
          getPlaylistTracksById(id)
            .then((tracks) => {
              if (selectedPlaylistIdRef.current === id) {
                setPlaylist(tracks);
              }
            })
            .catch(() => {});
        }
        loadPlaylists().catch(() => {});
      } catch (err) {
        failed++;
        console.warn(`Failed to sync playlist "${pl.name}":`, err);
      } finally {
        if (firstTimeFill) {
          if (selectedPlaylistIdRef.current === pl.id) {
            await loadPlaylistTracks(pl.id).catch(() => {});
          }
          endPlaylistImport(pl.id);
        }
      }
      // Let the UI process clicks between playlists.
      await new Promise((r) => setTimeout(r, 0));
    }
    stopProgress?.();
    setIsScanningFolder(false);
    setFolderScanIsSync(false);
    if (failed > 0) {
      setError(`Folder sync finished with ${failed} issue(s).`);
    } else if (isAndroidDevice && synced.length > 0) {
      // Drop legacy materialized copies after a successful URI re-sync.
      clearAudioImports().catch(() => {});
    }
  };

  const {
    lyricsPanelTrack,
    setLyricsPanelTrack,
    lyricsFullCover,
    activeLyricLineRef,
    lyricsFetchPath,
    cancelLyricsFetch,
    applyLyricsToTrack,
    timedLyrics,
    isLyricsPanelOnCurrentTrack,
    activeLyricIndex,
    lyricsScrollHandlers,
  } = useLyricsPanel({
    currentTrack,
    autoLyricsDownload,
    playbackCurrentPath: playbackState.current_path,
    displayPosition,
    setPlaylist,
    setQueueData,
  });

  const {
    sidebarWidth,
    rightPanelWidth,
    setRightPanelWidth,
    rightPanelOpen,
    rightPanelClosing,
    isMobileLayout,
    closeRightPanelDelayed,
    cancelCloseRightPanel,
    clampRightPanelWidth,
    onDragStart,
  } = useResizablePanels({
    panelOpen: showQueue || !!lyricsPanelTrack || showDeviceList,
    onCloseQueue: () => setShowQueue(false),
    onCloseDeviceList: () => setShowDeviceList(false),
    onCloseLyrics: () => setLyricsPanelTrack(null),
  });

  const playlistSearchPaths = useMemo(
    () => new Set(playlist.map((t) => t.path)),
    [playlist],
  );

  // Paths for Recently / Most Played so search stays scoped to those lists.
  const [playedSearchPaths, setPlayedSearchPaths] = useState<Set<string>>(
    () => new Set(),
  );
  useEffect(() => {
    if (mainView !== "recently_played" && mainView !== "most_played") {
      setPlayedSearchPaths(new Set());
      return;
    }
    let cancelled = false;
    const load =
      mainView === "recently_played" ? getRecentlyPlayed : getMostPlayed;
    void load(100)
      .then((tracks) => {
        if (!cancelled) {
          setPlayedSearchPaths(new Set(tracks.map((t) => t.path)));
        }
      })
      .catch(() => {
        if (!cancelled) setPlayedSearchPaths(new Set());
      });
    return () => {
      cancelled = true;
    };
  }, [mainView]);

  const albumScopeName = viewingAlbum?.name ?? null;
  const albumScopeArtist = viewingAlbum?.albumArtist ?? null;

  const mainSearchScope = useMemo((): MainSearchScope => {
    // Prefer browse stack over mainView so album/artist opened from Home
    // still scopes search (and can offer "Search full library").
    if (albumScopeName) {
      return {
        kind: "album",
        name: albumScopeName,
        albumArtist: albumScopeArtist,
      };
    }
    if (viewingArtist) {
      return { kind: "artist", name: viewingArtist };
    }
    if (mainView === "home" || mainView === "settings") {
      return { kind: "library" };
    }
    if (mainView === "recently_played" || mainView === "most_played") {
      return {
        kind: "playlist",
        label:
          mainView === "recently_played" ? "Recently Played" : "Most Played",
        paths: playedSearchPaths,
      };
    }
    const label = selectedPlaylist?.name ?? LIBRARY_PLAYLIST_NAME;
    if (isLibraryPlaylistName(label)) {
      return { kind: "library" };
    }
    return {
      kind: "playlist",
      label,
      paths: playlistSearchPaths,
    };
  }, [
    albumScopeName,
    albumScopeArtist,
    viewingArtist,
    mainView,
    selectedPlaylist?.name,
    playlistSearchPaths,
    playedSearchPaths,
  ]);

  const mainSearchScopeIsLibrary = mainSearchScope.kind === "library";

  // Stable key so object identity / path Set refreshes don't reset full-library mode.
  const mainSearchScopeKey =
    mainSearchScope.kind === "library"
      ? "library"
      : mainSearchScope.kind === "playlist"
        ? `playlist:${mainSearchScope.label}`
        : mainSearchScope.kind === "album"
          ? `album:${mainSearchScope.name}:${mainSearchScope.albumArtist ?? ""}`
          : `artist:${mainSearchScope.name}`;

  const displayedMainSearchHits = useMemo(() => {
    if (mainSearchFullLibrary || mainSearchScopeIsLibrary) {
      return mainSearchHits;
    }
    return mainSearchHits.filter((hit) =>
      hitMatchesSearchScope(hit, mainSearchScope),
    );
  }, [
    mainSearchHits,
    mainSearchFullLibrary,
    mainSearchScopeIsLibrary,
    mainSearchScope,
  ]);

  const showSearchFullLibraryBtn =
    !!mainSearchQuery.trim() &&
    !mainSearchFullLibrary &&
    !mainSearchScopeIsLibrary;

  const mainSearchResultsSubtitle = useMemo(() => {
    if (!mainSearchQuery.trim()) return "";
    if (mainSearchLoading && displayedMainSearchHits.length === 0) {
      return "Searching…";
    }
    const count = displayedMainSearchHits.length;
    const matchLabel = `${count} match${count === 1 ? "" : "es"}`;
    if (mainSearchFullLibrary || mainSearchScopeIsLibrary) {
      return matchLabel;
    }
    return `${matchLabel} in ${mainSearchScopeLabel(mainSearchScope)}`;
  }, [
    mainSearchQuery,
    mainSearchLoading,
    displayedMainSearchHits.length,
    mainSearchFullLibrary,
    mainSearchScopeIsLibrary,
    mainSearchScope,
  ]);

  useEffect(() => {
    setMainSearchFullLibrary(false);
  }, [mainSearchQuery, mainSearchScopeKey]);

  useEffect(() => {
    const initApp = async () => {
      await new Promise((resolve) => setTimeout(resolve, 300));
      try {
        const android = await isAndroid().catch(() => false);
        if (android) {
          try {
            const report = await takeAndroidCrashReport();
            if (report && report.trim()) {
              const trimmed = report.trim();
              // Safe-mode deferrals are not crashes — clear them silently.
              if (trimmed.startsWith("Wave soft failure")) {
                void clearAndroidCrashReport().catch(() => {});
              } else {
                setCrashReport(trimmed);
              }
            }
          } catch (crashErr) {
            console.warn("Crash report unavailable:", crashErr);
          }
        }
        setIsLoadingPlaylist(true);
        const list = await loadPlaylists();
        const defaultId = getDefaultPlaylistId(list);
        if (defaultId) {
          setActivePlaylistId(defaultId);
          await loadPlaylistTracks(defaultId);
        }
        await updatePlaybackState();
        await loadQueueTracks();
        await loadPlaybackMode();
        await loadEqSettings();
        await loadFavoritePaths();
        listOutputDevices().then(setOutputDevices).catch(console.error);

        // Reconcile synced playlists in the background — UI stays interactive.
        void syncFolderPlaylists(list, android);
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : undefined;
        if (
          message?.includes("not available") ||
          message?.includes("undefined")
        ) {
          setError(
            "Tauri API not available. Run `npm run tauri dev` instead of plain Vite.",
          );
        }
      } finally {
        setIsLoadingPlaylist(false);
      }
    };

    initApp();
    const interval = setInterval(
      () => updatePlaybackState().catch(() => {}),
      500,
    );
    const queueInterval = setInterval(
      () => loadQueueTracks().catch(() => {}),
      2000,
    );
    const modeInterval = setInterval(
      () => loadPlaybackMode().catch(() => {}),
      2000,
    );
    return () => {
      clearInterval(interval);
      clearInterval(queueInterval);
      clearInterval(modeInterval);
    };
  }, []);

  // Auto-advance is handled natively in Rust (`tick_auto_advance`) so the
  // queue keeps going on Android even when sink-empty detection is flaky.
  // Frontend only polls playback state for the UI.

  // When the playing track changes (including crossfade handoff), refresh the
  // queue highlight immediately instead of waiting for the slow poll.
  useEffect(() => {
    if (!playbackState.current_path) return;
    loadQueueTracks().catch(() => {});
  }, [playbackState.current_path]);

  // Poll queue more frequently while the panel is open
  useEffect(() => {
    if (!showQueue) return;
    loadQueueTracks().catch(() => {});
    const interval = setInterval(() => loadQueueTracks().catch(() => {}), 500);
    return () => clearInterval(interval);
  }, [showQueue]);

  useEffect(() => {
    if (!playlistDialog) return;
    playlistNameInputRef.current?.focus();
    playlistNameInputRef.current?.select();
  }, [playlistDialog]);

  // Push track metadata to OS media controls (Control Center, SMTC, MPRIS).
  // Fires on track change regardless of play state so the flyout updates immediately.
  // Cover art is omitted here on purpose — the Rust command fills in the 512px
  // media-session JPEG from the current track so we never overwrite OS art with
  // the 96px UI list thumb.
  useEffect(() => {
    if (currentTrack && playbackState.current_path) {
      updateMediaMetadata({
        title: currentTrack.title || currentTrack.name || "Unknown",
        artist: currentTrack.artist,
        album: currentTrack.album,
        duration_seconds: currentTrack.duration_seconds,
      }).catch(console.error);
    }
  }, [currentTrack?.path, playbackState.current_path]);

  // Listen for OS media control events (play/pause/next/prev/seek from OS).
  // Uses backend playback state directly — never opens the file picker.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    const osTogglePlayback = async () => {
      const state = await getPlaybackState();
      if (state.is_playing) {
        await pauseTrack();
      } else if (state.is_paused) {
        await resumeTrack();
      } else if (state.current_path) {
        await playTrack(state.current_path);
      } else {
        const list = await listPlaylists();
        const defaultId =
          list.find((p) => isLibraryPlaylistName(p.name))?.id ??
          list[0]?.id ??
          null;
        if (defaultId) {
          const tracks = await getPlaylistTracksById(defaultId);
          if (tracks.length > 0) {
            await playTrackFromSpecificPlaylist(defaultId, 0);
          }
        }
      }
      await updatePlaybackState();
    };

    const setup = async () => {
      const stop = await listenToMediaControls({
        onPlay: async () => {
          const state = await getPlaybackState();
          if (!state.is_playing) await osTogglePlayback();
        },
        onPause: async () => {
          const state = await getPlaybackState();
          if (state.is_playing) {
            await pauseTrack();
            await updatePlaybackState();
          }
        },
        onToggle: () => {
          osTogglePlayback().catch(console.error);
        },
        onNext: async () => {
          await playNext();
          await updatePlaybackState();
          await loadQueueTracks();
        },
        onPrevious: async () => {
          await playPrevious();
          await updatePlaybackState();
          await loadQueueTracks();
        },
        onStop: () => {
          handleStop().catch(console.error);
        },
        onSetPosition: async (seconds) => {
          const state = await getPlaybackState();
          if (state.current_path) {
            await seekTrack(seconds);
            await updatePlaybackState();
          }
        },
        onShuffle: async () => {
          const mode = await getPlaybackMode();
          await setShuffle(!mode.shuffle);
          await loadPlaybackMode();
        },
        onRepeat: async () => {
          const mode = await getPlaybackMode();
          const next =
            mode.repeat === "off"
              ? "all"
              : mode.repeat === "all"
                ? "one"
                : "off";
          await setRepeat(next);
          await loadPlaybackMode();
        },
      });
      // Registration finishes after the effect may already have torn down —
      // StrictMode does exactly that on every mount. Unsubscribing right away
      // in that case is what keeps a second listener from stacking up and
      // firing every OS media key twice.
      if (cancelled) stop();
      else unlisten = stop;
    };
    setup();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  // Cmd/Ctrl+K opens/focuses search; Escape clears or collapses it.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        openMainSearch();
      }
      if (event.key === "Escape" && (mainSearchQuery || mainSearchOpen)) {
        if (mainSearchQuery) setMainSearchQuery("");
        else closeMainSearch();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mainSearchQuery, mainSearchOpen]);

  // ── Playlist management ────────────────────────────────────────────────────

  const submitPlaylistDialog = async () => {
    if (!playlistDialog) return;

    const name = playlistNameInput.trim();
    if (!name) {
      setPlaylistDialogError("Enter a playlist name.");
      return;
    }

    try {
      setError(null);
      setPlaylistDialogError(null);

      if (playlistDialog.mode === "create") {
        const info = await createPlaylist(
          name,
          playlistSyncFolder || undefined,
        );
        await loadPlaylists();
        setActivePlaylistId(info.id);
        setMainView("playlist");
        await loadPlaylistTracks(info.id);
        if (playlistSyncFolder) {
          closePlaylistDialog();
          const paths = androidHost
            ? await scanDirectoryRecursive(playlistSyncFolder)
            : await scanDirectory(playlistSyncFolder);
          if (!paths.length) {
            setError(`No audio files found in the selected folder.`);
            return;
          }
          beginPlaylistImport(info.id);
          setIsScanningFolder(true);
          setFolderScanIsSync(true);
          setImportedCount(0);
          const stopProgress = await listenToSyncProgress((p) => {
            if (typeof p.processed === "number") setImportedCount(p.processed);
            else if (typeof p.extracted === "number")
              setImportedCount(p.extracted);
            else if (typeof p.added === "number") setImportedCount(p.added);
          }).catch(() => null);
          try {
            // Playlist was created with sync_folder — use the batched sync path.
            const syncResult = await syncPlaylistFolder(info.id, paths);
            if (syncResult.errors?.length) {
              setError(`Imported with ${syncResult.errors.length} error(s).`);
            }
            await loadPlaylists();
            await new Promise((r) => setTimeout(r, 0));
            await loadPlaylistTracks(info.id);
          } finally {
            stopProgress?.();
            endPlaylistImport(info.id);
            setIsScanningFolder(false);
            setFolderScanIsSync(false);
          }
          return;
        }
      } else {
        await renamePlaylist(playlistDialog.playlistId, name);
        await loadPlaylists();
      }

      closePlaylistDialog();
    } catch (err) {
      setPlaylistDialogError(formatInvokeError(err, "Failed to save playlist"));
    }
  };

  const handleSelectPlaylist = (id: string) => {
    const samePlaylist =
      selectedPlaylistIdRef.current === id && mainView === "playlist";
    clearBrowse();
    closeMainSearch();
    setMenuTrackPath(null);
    setMobileNavOpen(false);
    setMainView("playlist");

    // Already showing this playlist (e.g. leaving an album view) — no refetch flash.
    if (samePlaylist) {
      return;
    }

    setActivePlaylistId(id);
    // Clear immediately so the title and list never disagree while loading.
    setPlaylist([]);
    setIsLoadingPlaylist(true);

    void (async () => {
      try {
        // First-time import still running — keep the importing UI, don't flash
        // a partial track list from what's been written so far.
        if (importingPlaylistIdRef.current === id) {
          return;
        }
        await loadPlaylistTracks(id);
      } catch (err) {
        if (selectedPlaylistIdRef.current === id) {
          setError(formatInvokeError(err, "Failed to load playlist"));
        }
      } finally {
        if (selectedPlaylistIdRef.current === id) {
          setIsLoadingPlaylist(false);
        }
      }
    })();
  };

  const goHome = () => {
    clearBrowse();
    closeMainSearch();
    setMenuTrackPath(null);
    setMobileNavOpen(false);
    setMainView("home");
  };

  const goRecentlyPlayed = () => {
    clearBrowse();
    closeMainSearch();
    setMenuTrackPath(null);
    setMobileNavOpen(false);
    setMainView("recently_played");
  };

  const goMostPlayed = () => {
    clearBrowse();
    closeMainSearch();
    setMenuTrackPath(null);
    setMobileNavOpen(false);
    setMainView("most_played");
  };

  // ── Queue operations ───────────────────────────────────────────────────────

  const handleAddTrackToPlaylist = async (
    targetPlaylistId: string,
    path: string,
  ) => {
    try {
      setError(null);
      await addTrackToPlaylistById(targetPlaylistId, path);
      setAddToPlaylistTrack(null);
      setMenuTrackPath(null);
      await loadPlaylists();
      if (targetPlaylistId === selectedPlaylistId) {
        await loadPlaylistTracks(targetPlaylistId);
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to add track to playlist"));
    }
  };

  const openTrackContextMenu = (
    path: string,
    anchor: { top: number; right?: number; left?: number; flipAbove?: number },
  ) => {
    setQueueMenuIndex(null);
    setQueueMenuAnchor(null);
    setMenuTrackPath(path);
    setMenuAnchor(anchor);
    setAddToPlaylistTrack(null);
  };

  const closeTrackContextMenu = () => {
    setMenuTrackPath(null);
    setMenuAnchor(null);
  };

  // ── Export / Import ────────────────────────────────────────────────────────

  const handleExportLyrics = async (): Promise<string | null> => {
    try {
      setError(null);
      const path = await saveLyricsDialog();
      if (!path) return null;
      const count = await exportLyrics(path);
      return count === 0
        ? "No saved lyrics to export."
        : `Exported lyrics for ${count} track${count === 1 ? "" : "s"}.`;
    } catch (err) {
      setError(formatInvokeError(err, "Failed to export lyrics"));
      return null;
    }
  };

  const handleImportLyrics = async (): Promise<string | null> => {
    try {
      setError(null);
      const path = await openLyricsDialog();
      if (!path) return null;
      const result = await importLyrics(path);
      if (lyricsPanelTrack?.path) {
        const details = await getTrackDetails(lyricsPanelTrack.path);
        if (details?.lyrics) {
          applyLyricsToTrack(
            lyricsPanelTrack.path,
            details.lyrics,
            details.lyrics_source,
          );
        }
      }
      const parts = [
        `Imported ${result.imported}`,
        result.missing > 0 ? `${result.missing} not found` : null,
        result.skipped > 0 ? `${result.skipped} skipped` : null,
      ].filter(Boolean);
      return `${parts.join(" · ")}.`;
    } catch (err) {
      setError(formatInvokeError(err, "Failed to import lyrics"));
      return null;
    }
  };

  // ── Hardware/OS back button ─────────────────────────────────────────────
  // Push one history entry per navigation layer (album + now playing = two
  // backs to reach home). Transient overlays (menus, dialogs) add layers too.
  // On Android, keep a root guard entry for double-back-to-exit.
  const { showExitToast } = useAndroidBackTrap({
    androidHost,
    androidHostRef,
    showFolderSetup,
    skipFolderSetup,
    menuTrackPath,
    closeTrackContextMenu,
    queueMenuIndex,
    closeQueueContextMenu,
    showAddTrackMenu,
    setShowAddTrackMenu,
    setAddTrackMenuAnchor,
    showEqPanel,
    setShowEqPanel,
    setEqAnchor,
    mobilePlayerMenuOpen,
    setMobilePlayerMenuOpen,
    playlistDialog,
    closePlaylistDialog,
    showClearConfirm,
    setShowClearConfirm,
    showAddFromLibrary,
    closeAddFromLibrary,
    deletePlaylistConfirm,
    setDeletePlaylistConfirm,
    addToPlaylistTrack,
    setAddToPlaylistTrack,
    mobilePlayerView,
    setMobilePlayerView,
    rightPanelOpen,
    cancelCloseRightPanel,
    setShowQueue,
    setShowDeviceList,
    setLyricsPanelTrack,
    mobileSettingsOpen,
    mobileSettingsClosing,
    mobileSettingsClosingRef,
    forceCloseMobileSettings,
    handleCloseMobileSettings,
    mobilePlayerOpen,
    mobilePlayerClosing,
    mobilePlayerClosingRef,
    forceCloseMobilePlayer,
    handleCloseMobilePlayer,
    mobileNavOpen,
    setMobileNavOpen,
    mainSearchOpen,
    closeMainSearch,
    browseStackLength: browseStack.length,
    browseBack,
  });

  const isCurrentTrack = (track: Track) =>
    track.path === playbackState.current_path;

  useEffect(() => {
    void getSourceSettings()
      .then(setSourceSettingsState)
      .catch(() => {});
  }, []);

  const handleSourceSettingsChange = useCallback((next: SourceSettings) => {
    // Optimistic: the field stays responsive while the write lands.
    setSourceSettingsState(next);
    void setSourceSettings(next).catch((e) =>
      setError(e instanceof Error ? e.message : String(e)),
    );
  }, []);

  // A download that fell back to another folder must say so — silently
  // saving somewhere the user didn't choose is worse than not saving.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenToDownloadFallback((reason) => setError(reason))
      .then((fn) => {
        // Same teardown race as the media-control listener above: unsubscribe
        // immediately if the effect is already gone, or the toast fires once
        // per stacked registration.
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  /** Play a remote result, streaming it first unless the user already owns it. */
  const handleSourcePlay = useCallback(
    async (track: SourceTrack) => {
      try {
        // Owning it already makes the remote copy pointless — play the local file.
        const path = track.already_in_library
          ? track.already_in_library
          : (await streamSourceHit(track)).path;
        await playTracks([path], 0);
        updatePlaybackState();
        loadQueueTracks();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [streamSourceHit, updatePlaybackState, loadQueueTracks],
  );

  /** Keep a remote result in the library. */
  const handleSourceDownload = useCallback(
    async (track: SourceTrack) => {
      try {
        await downloadSourceHit(track);
        // The row is library content now, so the browse surfaces need it.
        await loadPlaylists();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [downloadSourceHit, loadPlaylists],
  );

  const mainSearchResultsPanel = (
    <div className="search-results">
      {mainSearchLoading && displayedMainSearchHits.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">
            <span className="import-spinner" />
          </div>
          <h2>Searching…</h2>
        </div>
      ) : displayedMainSearchHits.length === 0 ? (
        <div className="empty-state">
          <div className="empty-icon">
            <BiSearch />
          </div>
          <h2>No matches</h2>
          <p className="import-subtitle">
            {showSearchFullLibraryBtn
              ? `Nothing in ${mainSearchScopeLabel(mainSearchScope)} matched. Try searching the full library.`
              : "Try another song, artist, album, or lyric phrase."}
          </p>
          {showSearchFullLibraryBtn && (
            <button
              className="search-full-library-btn"
              type="button"
              onClick={() => setMainSearchFullLibrary(true)}
            >
              Search full library
            </button>
          )}
          {sourceSettings.outside_sourcing_enabled &&
            mainSearchQuery.trim() &&
            !sourceSearched &&
            !sourceLoading && (
              <button
                className="search-sources-btn"
                type="button"
                onClick={() => void searchSourcesNow()}
              >
                Search Deezer &amp; free catalogs
              </button>
            )}
          <SourceResults
            results={sourceResults}
            loading={sourceLoading}
            searched={sourceSearched}
            error={sourceError}
            busy={sourceBusy}
            query={mainSearchQuery}
            onPlay={(track) => void handleSourcePlay(track)}
            onDownload={(track) => void handleSourceDownload(track)}
          />
        </div>
      ) : (
        <>
          <div className="search-hit-list">
            {displayedMainSearchHits.map((hit) => {
              const track = hit.track;
              const fields = hit.matched_fields.filter(
                (f) => f in MATCH_FIELD_LABEL,
              );
              return (
                <button
                  key={track.id}
                  type="button"
                  className={`search-hit ${isCurrentTrack(track) ? "active" : ""}`}
                  onClick={() => {
                    const paths = displayedMainSearchHits.map(
                      (h) => h.track.path,
                    );
                    const index = Math.max(
                      0,
                      paths.findIndex((p) => p === track.path),
                    );
                    void playTracks(paths, index).then(() => {
                      updatePlaybackState();
                      loadQueueTracks();
                    });
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    openTrackContextMenu(track.path, {
                      top: event.clientY,
                      left: event.clientX,
                      flipAbove: event.clientY,
                    });
                  }}
                >
                  <Artwork
                    track={track}
                    fallback={getTrackTitle(track).slice(0, 1).toUpperCase()}
                    className="track-thumb search-hit-thumb"
                  />
                  <div className="search-hit-body">
                    <div className="search-hit-title">
                      {highlightMatch(getTrackTitle(track), mainSearchQuery)}
                    </div>
                    <div className="search-hit-meta">
                      {highlightMatch(track.artist, mainSearchQuery)}
                      {track.album ? (
                        <>
                          {" · "}
                          {highlightMatch(track.album, mainSearchQuery)}
                        </>
                      ) : null}
                    </div>
                    {hit.lyrics_snippet ? (
                      <div className="search-hit-lyrics">
                        {highlightMatch(hit.lyrics_snippet, mainSearchQuery)}
                      </div>
                    ) : null}
                    <div className="search-hit-fields">
                      {fields.map((field) => (
                        <span
                          key={field}
                          className={`search-field-chip search-field-${field}`}
                        >
                          {MATCH_FIELD_LABEL[field] ?? field}
                        </span>
                      ))}
                    </div>
                  </div>
                  <div className="search-hit-duration">
                    {formatTime(track.duration_seconds)}
                  </div>
                </button>
              );
            })}
          </div>
          {showSearchFullLibraryBtn && (
            <button
              className="search-full-library-btn"
              type="button"
              onClick={() => setMainSearchFullLibrary(true)}
            >
              Search full library
            </button>
          )}
          {sourceSettings.outside_sourcing_enabled &&
            mainSearchQuery.trim() &&
            !sourceSearched &&
            !sourceLoading && (
              <button
                className="search-sources-btn"
                type="button"
                onClick={() => void searchSourcesNow()}
              >
                Search Deezer &amp; free catalogs
              </button>
            )}
          <SourceResults
            results={sourceResults}
            loading={sourceLoading}
            searched={sourceSearched}
            error={sourceError}
            busy={sourceBusy}
            query={mainSearchQuery}
            onPlay={(track) => void handleSourcePlay(track)}
            onDownload={(track) => void handleSourceDownload(track)}
          />
        </>
      )}
    </div>
  );

  const coverLetters = getTrackTitle(currentTrack, playbackState.current_path)
    .slice(0, 2)
    .toUpperCase();

  return (
    <div
      className={`app-container${mobileNavOpen ? " nav-open" : ""}${rightPanelOpen || rightPanelClosing ? " panel-open" : ""}${rightPanelClosing ? " panel-closing" : ""}${mainSearchOpen ? " mobile-search-open" : ""}`}
      style={
        {
          "--sidebar-width": `${sidebarWidth}px`,
          "--right-panel-width":
            rightPanelOpen || rightPanelClosing
              ? `${rightPanelWidth}px`
              : "0px",
          "--right-handle-width":
            rightPanelOpen || rightPanelClosing
              ? "var(--section-inset)"
              : "0px",
        } as React.CSSProperties
      }
    >
      <header
        className={`mobile-topbar${mainSearchOpen ? " search-open" : ""}`}
      >
        <div className="mobile-topbar-row">
          <button
            className="mobile-topbar-btn"
            onClick={() => {
              setShowQueue(false);
              setShowDeviceList(false);
              setLyricsPanelTrack(null);
              closeMainSearch();
              setMobileNavOpen(true);
            }}
            type="button"
            title="Open playlists"
            aria-label="Open playlists"
          >
            <BiMenu />
          </button>
          <div className="mobile-topbar-title">
            <img src={trayTemplate} alt="Wave" className="mobile-topbar-logo" />
            {isScanningFolder || lyricsFetchPath ? (
              <span
                className="brand-sync-spinner"
                title={
                  isScanningFolder
                    ? folderScanIsSync
                      ? "Syncing folders…"
                      : "Importing…"
                    : "Fetching lyrics…"
                }
                aria-label={
                  isScanningFolder
                    ? folderScanIsSync
                      ? "Syncing folders"
                      : "Importing"
                    : "Fetching lyrics"
                }
                role="status"
              />
            ) : null}
          </div>
          <div className="mobile-topbar-actions">
            <button
              className={`mobile-topbar-btn ${mainSearchOpen ? "active" : ""}`}
              onClick={toggleMainSearch}
              type="button"
              title="Search"
              aria-label={mainSearchOpen ? "Close search" : "Search library"}
              aria-expanded={mainSearchOpen}
            >
              <BiSearch />
            </button>
          </div>
        </div>
        <div className="mobile-topbar-search" aria-hidden={!mainSearchOpen}>
          <div className="mobile-topbar-search-inner">
            <BiSearch className="library-search-icon" aria-hidden />
            <input
              ref={mobileSearchInputRef}
              className="library-search-input"
              type="search"
              placeholder="Search songs, artists, albums, lyrics…"
              value={mainSearchQuery}
              onChange={(e) => setMainSearchQuery(e.target.value)}
              aria-label="Search library"
              autoComplete="off"
              spellCheck={false}
              tabIndex={mainSearchOpen ? 0 : -1}
            />
            {mainSearchQuery ? (
              <button
                className="library-search-clear"
                type="button"
                onClick={() => setMainSearchQuery("")}
                title="Clear search"
                aria-label="Clear search"
                tabIndex={mainSearchOpen ? 0 : -1}
              >
                <BiX />
              </button>
            ) : (
              <button
                className="library-search-clear"
                type="button"
                onClick={closeMainSearch}
                title="Close search"
                aria-label="Close search"
                tabIndex={mainSearchOpen ? 0 : -1}
              >
                <BiX />
              </button>
            )}
          </div>
        </div>
      </header>

      <button
        className={`nav-backdrop${mobileNavOpen || rightPanelOpen || rightPanelClosing ? " nav-backdrop-open" : ""}${rightPanelClosing ? " nav-backdrop-closing" : ""}`}
        onClick={() => {
          setMobileNavOpen(false);
          closeRightPanelDelayed();
        }}
        type="button"
        aria-label="Close panel"
      />

      <Sidebar
        isScanningFolder={isScanningFolder}
        folderScanIsSync={folderScanIsSync}
        isBrowsing={!!viewingAlbum || !!viewingArtist}
        mainView={mainView}
        selectedPlaylistId={selectedPlaylistId}
        libraryPlaylist={libraryPlaylist}
        favoritesPlaylist={favoritesPlaylist}
        userPlaylists={userPlaylists}
        onGoHome={goHome}
        onGoRecentlyPlayed={goRecentlyPlayed}
        onGoMostPlayed={goMostPlayed}
        onSelectPlaylist={handleSelectPlaylist}
        onImportPlaylist={() => void handleImportPlaylist()}
        onCreatePlaylist={openCreatePlaylistDialog}
        onSyncPlaylist={(id) => void handleSyncPlaylistFolder(id)}
        onExportPlaylist={handleExportPlaylistById}
        onRenamePlaylist={openRenamePlaylistDialog}
        onDeletePlaylist={handleDeletePlaylist}
        onOpenSettings={handleOpenMobileSettings}
      />

      <div
        className="drag-handle drag-handle-sidebar"
        onMouseDown={onDragStart("sidebar")}
      />

      {mainSearchQuery.trim() && (viewingAlbum || viewingArtist) ? (
        <main className="main-content">
          <div className="hero-copy">
            <h1>Search</h1>
            <p>{mainSearchResultsSubtitle}</p>
          </div>
          <section className="playlist-container">
            {mainSearchResultsPanel}
          </section>
        </main>
      ) : viewingAlbum ? (
        <AlbumPage
          album={viewingAlbum.name}
          albumArtist={viewingAlbum.albumArtist}
          onBack={browseBack}
          onPlayTrack={(path, tracks) => {
            const index = Math.max(
              0,
              tracks.findIndex((t) => t.path === path),
            );
            void playTracks(
              tracks.map((t) => t.path),
              index,
            ).then(() => {
              updatePlaybackState();
              loadQueueTracks();
            });
          }}
          onArtistClick={(name) => {
            openArtistPage(name);
          }}
          playbackState={playbackState}
        />
      ) : viewingArtist ? (
        <ArtistPage
          artist={viewingArtist}
          onBack={browseBack}
          onPlayTrack={(path, tracks) => {
            const index = Math.max(
              0,
              tracks.findIndex((t) => t.path === path),
            );
            void playTracks(
              tracks.map((t) => t.path),
              index,
            ).then(() => {
              updatePlaybackState();
              loadQueueTracks();
            });
          }}
          onAlbumClick={(name, albumArtist) => {
            // Keep artist underneath so hardware/UI back returns to it.
            pushAlbumPage(name, albumArtist);
          }}
          playbackState={playbackState}
        />
      ) : mainView === "home" && !mainSearchQuery.trim() ? (
        <HomePage
          libraryPlaylistId={libraryPlaylist?.id ?? null}
          onPlayTrack={(path, tracks) => {
            const index = Math.max(
              0,
              tracks.findIndex((t) => t.path === path),
            );
            void playTracks(
              tracks.map((t) => t.path),
              index,
            ).then(() => {
              updatePlaybackState();
              loadQueueTracks();
            });
          }}
          onOpenAlbum={(name, albumArtist) => {
            openAlbumPage(name, albumArtist);
          }}
          onOpenArtist={(name) => {
            openArtistPage(name);
          }}
          onOpenLibrary={() => {
            if (libraryPlaylist) handleSelectPlaylist(libraryPlaylist.id);
          }}
        />
      ) : (mainView === "recently_played" || mainView === "most_played") &&
        !mainSearchQuery.trim() ? (
        <PlayedTracksPage
          mode={mainView}
          playbackState={playbackState}
          onPlayTrack={(path, tracks) => {
            const index = Math.max(
              0,
              tracks.findIndex((t) => t.path === path),
            );
            void playTracks(
              tracks.map((t) => t.path),
              index,
            ).then(() => {
              updatePlaybackState();
              loadQueueTracks();
            });
          }}
        />
      ) : mainView === "settings" && !mainSearchQuery.trim() ? (
        <MobileSettings
          embedded
          onClose={handleCloseMobileSettings}
          playlists={playlists}
          isScanningFolder={isScanningFolder}
          onCreatePlaylist={openCreatePlaylistDialog}
          onImportPlaylist={handleImportPlaylist}
          onRenamePlaylist={openRenamePlaylistDialog}
          onDeletePlaylist={handleDeletePlaylist}
          onExportPlaylist={handleExportPlaylistById}
          onSyncPlaylist={handleSyncPlaylistFolder}
          onAddMediaSource={handleAddMediaSource}
          onAddExtraMediaSource={handleAddExtraMediaSource}
          onRemoveMediaSource={handleRemoveMediaSource}
          onExportLyrics={handleExportLyrics}
          onImportLyrics={handleImportLyrics}
          autoLyricsDownload={autoLyricsDownload}
          onAutoLyricsDownloadChange={(enabled) =>
            void handleAutoLyricsDownloadChange(enabled)
          }
          eqSettings={eqSettings}
          onEqEnabledChange={handleEqEnabled}
          onEqBandChange={handleEqBandChange}
          onEqPreset={handleEqPreset}
          onEqReset={handleEqReset}
          crossfadeDuration={crossfadeDuration}
          onCrossfadeChange={handleCrossfadeChange}
          gaplessEnabled={gaplessEnabled}
          onGaplessChange={(enabled) => void handleGaplessChange(enabled)}
          volumeNormalizationEnabled={volumeNormalizationEnabled}
          onVolumeNormalizationChange={(enabled) =>
            void handleVolumeNormalizationChange(enabled)
          }
          sourceSettings={sourceSettings}
          onSourceSettingsChange={handleSourceSettingsChange}
          currentOutputDevice={playbackState.output_device_name}
          onSelectOutputDevice={handleSelectOutputDeviceSettings}
          onResetApp={handleResetApp}
        />
      ) : (
        <LibraryTrackList
          mainSearchQuery={mainSearchQuery}
          onMainSearchQueryChange={setMainSearchQuery}
          mainSearchOpen={mainSearchOpen}
          onOpenMainSearch={openMainSearch}
          onCloseMainSearch={closeMainSearch}
          mainSearchInputRef={mainSearchInputRef}
          mainSearchResultsSubtitle={mainSearchResultsSubtitle}
          mainSearchResultsPanel={mainSearchResultsPanel}
          selectedPlaylist={selectedPlaylist}
          playlist={playlist}
          sortedPlaylist={sortedPlaylist}
          isLoadingPlaylist={isLoadingPlaylist}
          isScanningFolder={isScanningFolder}
          isImporting={isImporting}
          isAddingTracks={isAddingTracks}
          importingPlaylistId={importingPlaylistId}
          selectedPlaylistId={selectedPlaylistId}
          importedCount={importedCount}
          playbackState={playbackState}
          androidHost={androidHost}
          addTrackBtnRef={addTrackBtnRef}
          trackGridCols={trackGridCols}
          sortColumn={sortColumn}
          sortDirection={sortDirection}
          favoritePaths={favoritePaths}
          menuTrackPath={menuTrackPath}
          onPlayPause={handlePlayPause}
          onAddFolderAndroid={() => void handleAddFolderAndroid()}
          onOpenAddFromLibrary={openAddFromLibrary}
          onOpenAddTrackMenu={() => {
            if (addTrackBtnRef.current) {
              const rect = addTrackBtnRef.current.getBoundingClientRect();
              setAddTrackMenuAnchor({ top: rect.bottom + 6, left: rect.left });
            }
            setShowAddTrackMenu((v) => !v);
          }}
          onAddTrack={() => void handleAddTrack(false)}
          onClearPlaylist={handleClearPlaylist}
          onSort={handleSort}
          onResizeAlbumColumn={handleAlbumColResizeStart}
          onSyncPlaylist={(id) => void handleSyncPlaylistFolder(id)}
          isCurrentTrack={isCurrentTrack}
          onPlayTrack={handlePlayTrack}
          onOpenArtist={openArtistPage}
          onOpenAlbum={openAlbumPage}
          onOpenTrackContextMenu={openTrackContextMenu}
          onCloseTrackMenu={closeTrackContextMenu}
          onRemoveFromPlaylist={(path) => void handleRemoveFromPlaylist(path)}
          onRemoveFromLibrary={(path) => void handleRemoveFromLibrary(path)}
          onToggleFavorite={handleToggleFavorite}
        />
      )}

      {(rightPanelOpen || rightPanelClosing) && (
        <div
          className="drag-handle drag-handle-right"
          onMouseDown={onDragStart("right")}
        />
      )}

      <aside className="right-panel">
        {showQueue && (
          <QueuePanel
            tracks={queueData.tracks}
            currentIndex={queueData.current_index}
            menuIndex={queueMenuIndex}
            onClose={closeRightPanelDelayed}
            onClear={handleClearQueue}
            onPlayFromQueue={handlePlayFromQueue}
            onOpenMenu={openQueueContextMenu}
            onCloseMenu={closeQueueContextMenu}
            onRemove={handleRemoveFromQueue}
          />
        )}
        {lyricsPanelTrack && (
          <LyricsPanel
            track={lyricsPanelTrack}
            fullCover={lyricsFullCover}
            onClose={closeRightPanelDelayed}
            onOpenArtist={openArtistPage}
            onOpenAlbum={openAlbumPage}
            timedLyrics={timedLyrics}
            activeLyricIndex={activeLyricIndex}
            activeLyricLineRef={activeLyricLineRef}
            isCurrentTrack={isLyricsPanelOnCurrentTrack}
            onSeek={(time) => void handleSeek(time)}
            scrollHandlers={lyricsScrollHandlers}
          />
        )}
        {showDeviceList && (
          <DeviceListPanel
            devices={outputDevices}
            currentDeviceName={playbackState.output_device_name}
            onClose={closeRightPanelDelayed}
            onSelectDevice={(name) => {
              void (async () => {
                try {
                  await setOutputDevice(name);
                  await updatePlaybackState();
                  setShowDeviceList(false);
                } catch (err) {
                  setError(
                    err instanceof Error
                      ? err.message
                      : "Failed to change audio device",
                  );
                  setShowDeviceList(false);
                }
              })();
            }}
          />
        )}
      </aside>

      {showAddTrackMenu && addTrackMenuAnchor && (
        <AddTrackMenu
          anchor={addTrackMenuAnchor}
          onClose={() => {
            setShowAddTrackMenu(false);
            setAddTrackMenuAnchor(null);
          }}
          androidHost={androidHost}
          onAddFiles={() => void handleAddTrack(true)}
          onAddFolder={() => void handleAddFolder()}
          onAddFolderAsPlaylist={() => void handleAddFolderAsPlaylist()}
        />
      )}

      {menuTrackPath &&
        menuAnchor &&
        (() => {
          const menuTrack = playlist.find((t) => t.path === menuTrackPath);
          if (!menuTrack) return null;
          const addToPlaylistOptions = playlists.filter(
            (p) => p.id !== selectedPlaylistId && p.name !== "Favorites",
          );
          return (
            <TrackContextMenu
              track={menuTrack}
              anchor={menuAnchor}
              onClose={closeTrackContextMenu}
              canAddToPlaylist={addToPlaylistOptions.length > 0}
              canRemoveFromPlaylist={
                !isLibraryPlaylistName(selectedPlaylist?.name)
              }
              onPlayNext={handlePlayNext}
              onAddToQueue={handleAddToQueue}
              onAddToPlaylist={setAddToPlaylistTrack}
              onGoToAlbum={openAlbumPage}
              onGoToArtist={openArtistPage}
              onRemoveFromPlaylist={(path) =>
                void handleRemoveFromPlaylist(path)
              }
              onRemoveFromLibrary={(path) => void handleRemoveFromLibrary(path)}
            />
          );
        })()}

      {queueMenuIndex != null && queueMenuAnchor && (
        <QueueContextMenu
          index={queueMenuIndex}
          queueLength={queueData.tracks.length}
          anchor={queueMenuAnchor}
          onClose={closeQueueContextMenu}
          onMove={handleMoveQueueTrack}
          onRemove={handleRemoveFromQueue}
        />
      )}

      {playlistDialog && (
        <CreatePlaylistDialog
          dialog={playlistDialog}
          onClose={closePlaylistDialog}
          onSubmit={submitPlaylistDialog}
          nameInputRef={playlistNameInputRef}
          name={playlistNameInput}
          onNameChange={setPlaylistNameInput}
          androidHost={androidHost}
          syncFolder={playlistSyncFolder}
          onClearSyncFolder={() => setPlaylistSyncFolderInput(null)}
          onPickSyncFolder={() => void pickPlaylistSyncFolder()}
          error={playlistDialogError}
        />
      )}

      {showClearConfirm && (
        <ClearPlaylistDialog
          onCancel={() => setShowClearConfirm(false)}
          onConfirm={confirmClearPlaylist}
        />
      )}

      {showAddFromLibrary && (
        <AddFromLibraryDialog
          onClose={closeAddFromLibrary}
          query={librarySearchQuery}
          onQueryChange={setLibrarySearchQuery}
          loading={librarySearchLoading}
          adding={librarySearchAdding}
          results={librarySearchResults}
          selected={librarySearchSelected}
          onToggleSelect={toggleLibrarySearchSelect}
          onPickFile={() => void handlePickFileFromLibraryModal()}
          onAddSelected={() => void handleAddSelectedFromLibrary()}
        />
      )}

      {deletePlaylistConfirm && (
        <DeletePlaylistDialog
          name={deletePlaylistConfirm.name}
          onCancel={() => setDeletePlaylistConfirm(null)}
          onConfirm={confirmDeletePlaylist}
        />
      )}

      {addToPlaylistTrack && (
        <AddToPlaylistDialog
          playlists={playlists}
          excludePlaylistId={selectedPlaylistId}
          onClose={() => setAddToPlaylistTrack(null)}
          onSelect={(playlistId) =>
            handleAddTrackToPlaylist(playlistId, addToPlaylistTrack)
          }
        />
      )}

      <PlayerBar
        currentTrack={currentTrack}
        playbackState={playbackState}
        playbackMode={playbackMode}
        displayPosition={displayPosition}
        displayDuration={displayDuration}
        canSkip={canSkip}
        showQueue={showQueue}
        showEqPanel={showEqPanel}
        eqSettings={eqSettings}
        volumeValue={volumeValue}
        lyricsPanelTrack={lyricsPanelTrack}
        coverLetters={coverLetters}
        isMobileLayout={isMobileLayout}
        mobilePlayerOpenRef={mobilePlayerOpenRef}
        mobilePlayerClosingRef={mobilePlayerClosingRef}
        mobilePlayerClosing={mobilePlayerClosing}
        volumeIconRef={volumeIconRef}
        onOpenMobilePlayer={handleOpenMobilePlayer}
        onOpenNowPlaying={handleOpenNowPlaying}
        onOpenArtist={openArtistPage}
        onOpenAlbum={openAlbumPage}
        onToggleShuffle={handleToggleShuffle}
        onPrevious={handlePrevious}
        onStop={handleStop}
        onPlayPause={handlePlayPause}
        onNext={handleNext}
        onCycleRepeat={handleCycleRepeat}
        onSeekChange={setSeekValue}
        onSeekCommit={(value) => void handleSeek(value)}
        onToggleLyrics={handleToggleLyrics}
        onToggleQueue={handleToggleQueue}
        onToggleEqPanel={() => void handleToggleEqPanel()}
        onVolumeChange={(value) => void handleVolume(value)}
        onToggleDevice={handleToggleDevice}
        onRefreshOutputDevices={() =>
          listOutputDevices().then(setOutputDevices).catch(console.error)
        }
      />

      {mobilePlayerOpen && currentTrack && (
        <MobileNowPlaying
          key={mobilePlayerKey}
          track={currentTrack}
          isPlaying={playbackState.is_playing}
          isFavorite={favoritePaths.has(currentTrack.path)}
          onToggleFavorite={() => handleToggleFavorite(currentTrack.path)}
          displayPosition={displayPosition}
          displayDuration={displayDuration}
          onSeekChange={setSeekValue}
          onSeekCommit={handleSeek}
          playbackMode={playbackMode}
          canSkip={canSkip}
          onPlayPause={handlePlayPause}
          onPrevious={handlePrevious}
          onNext={handleNext}
          onToggleShuffle={handleToggleShuffle}
          onCycleRepeat={handleCycleRepeat}
          closing={mobilePlayerClosing}
          onClose={handleCloseMobilePlayer}
          onDragClose={handleDragCloseMobilePlayer}
          onOpenArtist={(name) => {
            armDragDismissGhostClickGuard();
            forceCloseMobilePlayer();
            openArtistPage(name);
          }}
          onOpenAlbum={(name, albumArtist) => {
            armDragDismissGhostClickGuard();
            forceCloseMobilePlayer();
            openAlbumPage(name, albumArtist);
          }}
          volumeValue={volumeValue}
          onVolumeChange={handleVolume}
          hideVolume={androidHost}
          eqSettings={eqSettings}
          onEqEnabledChange={handleEqEnabled}
          onEqBandChange={handleEqBandChange}
          onEqBandsChange={handleEqBandsChange}
          onEqPreset={handleEqPreset}
          onEqReset={handleEqReset}
          view={mobilePlayerView}
          onViewChange={setMobilePlayerView}
          menuOpen={mobilePlayerMenuOpen}
          onMenuOpenChange={setMobilePlayerMenuOpen}
          queueTracks={queueData.tracks}
          queueCurrentIndex={queueData.current_index}
          onPlayFromQueue={handlePlayFromQueue}
          onRemoveFromQueue={handleRemoveFromQueue}
          onReorderQueue={handleMoveQueueTrack}
          onClearQueue={handleClearQueue}
        />
      )}

      {mobileSettingsOpen && (
        <MobileSettings
          closing={mobileSettingsClosing}
          onClose={handleCloseMobileSettings}
          playlists={playlists}
          isScanningFolder={isScanningFolder}
          onCreatePlaylist={openCreatePlaylistDialog}
          onImportPlaylist={handleImportPlaylist}
          onRenamePlaylist={openRenamePlaylistDialog}
          onDeletePlaylist={handleDeletePlaylist}
          onExportPlaylist={handleExportPlaylistById}
          onSyncPlaylist={handleSyncPlaylistFolder}
          onAddMediaSource={handleAddMediaSource}
          onAddExtraMediaSource={handleAddExtraMediaSource}
          onRemoveMediaSource={handleRemoveMediaSource}
          onExportLyrics={handleExportLyrics}
          onImportLyrics={handleImportLyrics}
          autoLyricsDownload={autoLyricsDownload}
          onAutoLyricsDownloadChange={(enabled) =>
            void handleAutoLyricsDownloadChange(enabled)
          }
          eqSettings={eqSettings}
          onEqEnabledChange={handleEqEnabled}
          onEqBandChange={handleEqBandChange}
          onEqPreset={handleEqPreset}
          onEqReset={handleEqReset}
          crossfadeDuration={crossfadeDuration}
          onCrossfadeChange={handleCrossfadeChange}
          gaplessEnabled={gaplessEnabled}
          onGaplessChange={(enabled) => void handleGaplessChange(enabled)}
          volumeNormalizationEnabled={volumeNormalizationEnabled}
          onVolumeNormalizationChange={(enabled) =>
            void handleVolumeNormalizationChange(enabled)
          }
          sourceSettings={sourceSettings}
          onSourceSettingsChange={handleSourceSettingsChange}
          currentOutputDevice={playbackState.output_device_name}
          onSelectOutputDevice={handleSelectOutputDeviceSettings}
          onResetApp={handleResetApp}
        />
      )}

      {showEqPanel && eqAnchor && (
        <EqPanel
          anchor={eqAnchor}
          onClose={() => {
            setShowEqPanel(false);
            setEqAnchor(null);
          }}
          settings={eqSettings}
          onEnabledChange={handleEqEnabled}
          onPresetSelect={(presetId) => void handleEqPreset(presetId)}
          onReset={handleEqReset}
          onBandChange={handleEqBandChange}
          crossfadeDuration={crossfadeDuration}
          onCrossfadeChange={handleCrossfadeChange}
          gaplessEnabled={gaplessEnabled}
          onGaplessChange={(enabled) => void handleGaplessChange(enabled)}
        />
      )}

      {showFolderSetup && androidHost && (
        <div className="modal-backdrop" onClick={() => {}}>
          <div className="modal-dialog" onClick={(e) => e.stopPropagation()}>
            <h2>Welcome to Wave</h2>
            <p className="modal-desc">
              Select a folder containing your music to get started. Wave indexes
              the files in place (no copies) and lists them in your Library.
            </p>
            <div className="modal-actions">
              <button
                className="btn-ghost"
                onClick={() => void skipFolderSetup()}
                type="button"
              >
                Skip for now
              </button>
              <button
                className="btn-primary"
                onClick={() => void handleAddFolderAndroid()}
                type="button"
              >
                <BiFolderOpen /> Select Media Source
              </button>
            </div>
          </div>
        </div>
      )}

      <StatusToasts
        showExitToast={showExitToast}
        crashReport={crashReport}
        onDismissCrashReport={() => {
          void clearAndroidCrashReport().catch(() => {});
          setCrashReport(null);
        }}
        error={error}
        onDismissError={() => setError(null)}
        lyricsFetchPath={lyricsFetchPath}
        onCancelLyricsFetch={cancelLyricsFetch}
        isLoading={isLoading}
      />
    </div>
  );
}

export default App;
