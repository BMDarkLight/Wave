import { useEffect, useRef, useState } from "react";
import {
  addTrackToPlaylistById,
  clearAudioImports,
  createPlaylist,
  getFileName,
  importScannedAudio,
  listenToSyncProgress,
  removeMediaFolder,
  removeTrackFromLibrary,
  removeTrackFromPlaylistById,
  saveMediaFolder,
  scanDirectory,
  scanDirectoryRecursive,
  searchLibraryTracks,
  selectAudioFile,
  selectAudioFolder,
  selectMediaFolder,
  setOutputDevice,
  setPlaylistSyncFolder,
  stopTrack,
  syncPlaylistFolder,
  type PlaybackState,
  type PlaylistInfo,
  type Track,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";
import { isLibraryPlaylistName } from "../utils/track";

/** Adding tracks/folders to the library: the add-track menu, the "add from
 * library" picker, folder scan + sync (including the first-time import
 * progress UI), media-source management, and track removal. */
export function useMediaImport({
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
}: {
  androidHost: boolean;
  selectedPlaylistId: string | null;
  selectedPlaylistIdRef: { current: string | null };
  selectedPlaylist: PlaylistInfo | null;
  playlists: PlaylistInfo[];
  loadPlaylists: () => Promise<PlaylistInfo[]>;
  loadPlaylistTracks: (playlistId: string) => Promise<boolean>;
  getDefaultPlaylistId: (list: PlaylistInfo[]) => string | null;
  setActivePlaylistId: (id: string | null) => void;
  setMainView: (view: "playlist") => void;
  setError: (message: string | null) => void;
  setShowFolderSetup: (show: boolean) => void;
  playbackState: PlaybackState;
  setSeekValue: (value: number) => void;
  loadQueueTracks: () => Promise<void>;
  updatePlaybackState: () => Promise<void>;
  loadFavoritePaths: () => Promise<void>;
}) {
  const [isAddingTracks, setIsAddingTracks] = useState(false);
  /** Playlist currently undergoing a first-time folder import (blocks list UI). */
  const [importingPlaylistId, setImportingPlaylistId] = useState<string | null>(null);
  const importingPlaylistIdRef = useRef<string | null>(null);
  const isImporting = importingPlaylistId != null;
  const [importedCount, setImportedCount] = useState(0);

  const beginPlaylistImport = (playlistId: string) => {
    importingPlaylistIdRef.current = playlistId;
    setImportingPlaylistId(playlistId);
    setImportedCount(0);
  };

  const endPlaylistImport = (playlistId?: string | null) => {
    if (
      playlistId != null &&
      importingPlaylistIdRef.current != null &&
      importingPlaylistIdRef.current !== playlistId
    ) {
      return;
    }
    importingPlaylistIdRef.current = null;
    setImportingPlaylistId(null);
  };

  const [showAddTrackMenu, setShowAddTrackMenu] = useState(false);
  const [addTrackMenuAnchor, setAddTrackMenuAnchor] = useState<{
    top: number;
    left: number;
  } | null>(null);

  const [showAddFromLibrary, setShowAddFromLibrary] = useState(false);
  const [librarySearchQuery, setLibrarySearchQuery] = useState("");
  const [librarySearchResults, setLibrarySearchResults] = useState<Track[]>([]);
  const [librarySearchSelected, setLibrarySearchSelected] = useState<Set<string>>(new Set());
  const [librarySearchLoading, setLibrarySearchLoading] = useState(false);
  const [librarySearchAdding, setLibrarySearchAdding] = useState(false);
  const librarySearchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [isScanningFolder, setIsScanningFolder] = useState(false);
  const [folderScanIsSync, setFolderScanIsSync] = useState(false);

  const handleAddTrack = async (multiple = false) => {
    try {
      setError(null);
      // Desktop: close the menu first so the native dialog isn't blocked by the
      // overlay/history sentinel. Android needs the picker in the same gesture
      // chain, so the menu stays open until after the picker returns.
      if (!androidHost) {
        setShowAddTrackMenu(false);
        setAddTrackMenuAnchor(null);
      }
      const paths = await selectAudioFile(multiple);
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      if (!paths?.length) {
        return;
      }
      setIsAddingTracks(true);
      const playlistId = selectedPlaylistId ?? getDefaultPlaylistId(playlists);
      if (!playlistId) {
        setError("No playlist selected.");
        return;
      }
      let failCount = 0;
      for (const path of paths) {
        try {
          await addTrackToPlaylistById(playlistId, path);
        } catch (err) {
          failCount++;
          console.error("Failed to add track:", path, err);
        }
      }
      if (failCount > 0) setError(`Failed to add ${failCount} track(s).`);
      await loadPlaylistTracks(playlistId);
      await loadPlaylists();
    } catch (err) {
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      setError(formatInvokeError(err, "Failed to add track"));
    } finally {
      setIsAddingTracks(false);
    }
  };

  const openAddFromLibrary = () => {
    setLibrarySearchQuery("");
    setLibrarySearchResults([]);
    setLibrarySearchSelected(new Set());
    setShowAddFromLibrary(true);
  };

  const closeAddFromLibrary = () => {
    setShowAddFromLibrary(false);
    setLibrarySearchQuery("");
    setLibrarySearchResults([]);
    setLibrarySearchSelected(new Set());
    setLibrarySearchLoading(false);
    setLibrarySearchAdding(false);
    if (librarySearchTimer.current) {
      clearTimeout(librarySearchTimer.current);
      librarySearchTimer.current = null;
    }
  };

  useEffect(() => {
    if (!showAddFromLibrary) return;
    if (librarySearchTimer.current) clearTimeout(librarySearchTimer.current);
    librarySearchTimer.current = setTimeout(() => {
      const q = librarySearchQuery;
      setLibrarySearchLoading(true);
      searchLibraryTracks(q, 80)
        .then((tracks) => {
          setLibrarySearchResults(tracks);
          setLibrarySearchSelected((prev) => {
            const next = new Set<string>();
            for (const path of prev) {
              if (tracks.some((t) => t.path === path)) next.add(path);
            }
            return next;
          });
        })
        .catch(() => setLibrarySearchResults([]))
        .finally(() => setLibrarySearchLoading(false));
    }, 200);
    return () => {
      if (librarySearchTimer.current) {
        clearTimeout(librarySearchTimer.current);
        librarySearchTimer.current = null;
      }
    };
  }, [showAddFromLibrary, librarySearchQuery]);

  const toggleLibrarySearchSelect = (path: string) => {
    setLibrarySearchSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const handleAddSelectedFromLibrary = async () => {
    const playlistId = selectedPlaylistId ?? getDefaultPlaylistId(playlists);
    if (!playlistId || librarySearchSelected.size === 0) return;
    setLibrarySearchAdding(true);
    setError(null);
    let failCount = 0;
    for (const path of librarySearchSelected) {
      try {
        await addTrackToPlaylistById(playlistId, path);
      } catch (err) {
        failCount++;
        console.error("Failed to add library track:", path, err);
      }
    }
    setLibrarySearchAdding(false);
    if (failCount > 0) setError(`Failed to add ${failCount} track(s).`);
    closeAddFromLibrary();
    await loadPlaylistTracks(playlistId);
    await loadPlaylists();
  };

  const handlePickFileFromLibraryModal = async () => {
    try {
      setError(null);
      const paths = await selectAudioFile(true);
      if (!paths?.length) return;
      const playlistId = selectedPlaylistId ?? getDefaultPlaylistId(playlists);
      if (!playlistId) {
        setError("No playlist selected.");
        return;
      }
      setLibrarySearchAdding(true);
      let failCount = 0;
      for (const path of paths) {
        try {
          await addTrackToPlaylistById(playlistId, path);
        } catch (err) {
          failCount++;
          console.error("Failed to add picked track:", path, err);
        }
      }
      setLibrarySearchAdding(false);
      if (failCount > 0) setError(`Failed to add ${failCount} track(s).`);
      closeAddFromLibrary();
      await loadPlaylistTracks(playlistId);
      await loadPlaylists();
    } catch (err) {
      setLibrarySearchAdding(false);
      setError(formatInvokeError(err, "Failed to pick audio file"));
    }
  };

  const runFolderImport = async (paths: string[], playlistId: string) => {
    beginPlaylistImport(playlistId);
    setIsScanningFolder(true);
    setFolderScanIsSync(false);
    setImportedCount(0);
    const stopProgress = await listenToSyncProgress((p) => {
      if (typeof p.processed === "number") setImportedCount(p.processed);
      else if (typeof p.extracted === "number") setImportedCount(p.extracted);
      else if (typeof p.added === "number") setImportedCount(p.added);
    }).catch(() => null);

    try {
      // One bulk IPC: extract outside the DB lock in Rust batches.
      const result = await importScannedAudio(paths, playlistId);
      if (result.errors?.length) {
        setError(`Finished importing folder with ${result.errors.length} failure(s).`);
      }
      if (selectedPlaylistIdRef.current === playlistId) {
        await loadPlaylists();
        await new Promise((r) => setTimeout(r, 0));
        await loadPlaylistTracks(playlistId);
      } else {
        await loadPlaylists();
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to import folder"));
    } finally {
      stopProgress?.();
      endPlaylistImport(playlistId);
      setIsScanningFolder(false);
    }
  };

  const handleAddFolder = async () => {
    try {
      setError(null);
      // Desktop-only path: close menu before opening the folder dialog.
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      const directory = await selectAudioFolder();
      if (!directory) {
        return;
      }
      setIsAddingTracks(true);
      const paths = await scanDirectory(directory);
      if (!paths.length) {
        setError("No audio files found in the selected folder.");
        setIsAddingTracks(false);
        return;
      }
      const playlistId = selectedPlaylistId ?? getDefaultPlaylistId(playlists);
      if (!playlistId) {
        setError("No playlist selected.");
        setIsAddingTracks(false);
        return;
      }
      setIsAddingTracks(false);
      runFolderImport(paths, playlistId).catch(() => {});
    } catch (err) {
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      setError(formatInvokeError(err, "Failed to add folder"));
      setIsAddingTracks(false);
    }
  };

  const handleAddFolderAndroid = async () => {
    try {
      setError(null);
      // Keep the user-gesture chain intact for the SAF folder picker.
      let result;
      try {
        result = await selectMediaFolder();
      } catch (err) {
        setError(formatInvokeError(err, "Failed to open folder picker"));
        return;
      }
      if (!result?.uri) return;
      setShowFolderSetup(false);

      const directory = result.uri;

      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);

      const paths = await scanDirectoryRecursive(directory);
      if (!paths.length) {
        setError("No audio files found in the selected folder.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }

      // Android media scan always targets Library.
      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const playlistId =
        list.find((p) => isLibraryPlaylistName(p.name))?.id ?? getDefaultPlaylistId(list);
      if (!playlistId) {
        setError("No playlist selected.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }

      await setPlaylistSyncFolder(playlistId, directory);
      await saveMediaFolder(directory).catch(() => {});

      beginPlaylistImport(playlistId);
      const stopProgress = await listenToSyncProgress((p) => {
        if (typeof p.processed === "number") setImportedCount(p.processed);
        else if (typeof p.extracted === "number") setImportedCount(p.extracted);
        else if (typeof p.added === "number") setImportedCount(p.added);
      }).catch(() => null);

      try {
        const syncResult = await syncPlaylistFolder(playlistId, paths);
        if (syncResult.errors?.length) {
          setError(
            `Imported with ${syncResult.errors.length} error(s) — ${syncResult.errors.slice(0, 2).join(" | ")}`,
          );
        } else {
          clearAudioImports().catch(() => {});
        }
      } finally {
        stopProgress?.();
        endPlaylistImport(playlistId);
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
      }

      setActivePlaylistId(playlistId);
      setMainView("playlist");
      // Defer the full track list load so the import overlay can clear first.
      await loadPlaylists();
      await new Promise((r) => setTimeout(r, 0));
      await loadPlaylistTracks(playlistId);
    } catch (err) {
      endPlaylistImport();
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
      setError(formatInvokeError(err, "Failed to scan folder"));
    }
  };

  /** Mobile Settings "Media Source Folders" — Android uses the SAF folder
   *  picker; a desktop window narrowed into the mobile layout falls back to
   *  the regular directory picker, mirroring handleAddFolderAndroid. */
  const handleAddMediaSource = async () => {
    if (androidHost) {
      await handleAddFolderAndroid();
      return;
    }
    try {
      setError(null);
      const directory = await selectAudioFolder();
      if (!directory) return;
      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);
      const paths = await scanDirectory(directory);
      if (!paths.length) {
        setError("No audio files found in the selected folder.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }
      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const playlistId =
        list.find((p) => isLibraryPlaylistName(p.name))?.id ?? getDefaultPlaylistId(list);
      if (!playlistId) {
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }
      await setPlaylistSyncFolder(playlistId, directory);
      await saveMediaFolder(directory).catch(() => {});
      beginPlaylistImport(playlistId);
      const stopProgress = await listenToSyncProgress((p) => {
        if (typeof p.processed === "number") setImportedCount(p.processed);
        else if (typeof p.extracted === "number") setImportedCount(p.extracted);
        else if (typeof p.added === "number") setImportedCount(p.added);
      }).catch(() => null);
      try {
        await syncPlaylistFolder(playlistId, paths);
        setActivePlaylistId(playlistId);
        setMainView("playlist");
        await loadPlaylists();
        await new Promise((r) => setTimeout(r, 0));
        await loadPlaylistTracks(playlistId);
      } finally {
        stopProgress?.();
        endPlaylistImport(playlistId);
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
      }
    } catch (err) {
      endPlaylistImport();
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
      setError(formatInvokeError(err, "Failed to add media folder"));
    }
  };

  /** Add another media folder into Library without replacing the Library sync folder. */
  const handleAddExtraMediaSource = async () => {
    try {
      setError(null);
      let directory: string | null = null;
      if (androidHost) {
        let result;
        try {
          result = await selectMediaFolder();
        } catch (err) {
          setError(formatInvokeError(err, "Failed to open folder picker"));
          return;
        }
        if (!result?.uri) return;
        directory = result.uri;
      } else {
        directory = await selectAudioFolder();
        if (!directory) return;
      }

      setIsScanningFolder(true);
      setFolderScanIsSync(false);
      setImportedCount(0);

      const paths = androidHost
        ? await scanDirectoryRecursive(directory)
        : await scanDirectory(directory);
      if (!paths.length) {
        setError("No audio files found in the selected folder.");
        setIsScanningFolder(false);
        return;
      }

      await saveMediaFolder(directory).catch(() => {});

      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const playlistId =
        list.find((p) => isLibraryPlaylistName(p.name))?.id ?? getDefaultPlaylistId(list);
      if (!playlistId) {
        setError("No playlist selected.");
        setIsScanningFolder(false);
        return;
      }

      await runFolderImport(paths, playlistId);
    } catch (err) {
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
      setError(formatInvokeError(err, "Failed to add media source"));
    }
  };

  /** Forget a media source folder; also unbind Library (or any playlist) synced to it. */
  const handleRemoveMediaSource = async (path: string) => {
    try {
      setError(null);
      await removeMediaFolder(path);
      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const bound = list.filter((p) => p.sync_folder === path);
      for (const pl of bound) {
        await setPlaylistSyncFolder(pl.id, null);
      }
      if (bound.length > 0) {
        await loadPlaylists();
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to remove media source"));
    }
  };

  const handleSelectOutputDeviceSettings = async (name: string) => {
    try {
      await setOutputDevice(name);
      await updatePlaybackState();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to change audio device"));
    }
  };

  const handleAddFolderAsPlaylist = async () => {
    try {
      setError(null);
      // Desktop-only: close menu, then bind sync_folder and import.
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      const directory = await selectAudioFolder();
      if (!directory) {
        return;
      }
      setIsAddingTracks(true);
      const folderName = getFileName(directory);
      const info = await createPlaylist(folderName, directory);
      setActivePlaylistId(info.id);
      setMainView("playlist");
      await loadPlaylists();
      await loadPlaylistTracks(info.id);
      const paths = await scanDirectory(directory);
      setIsAddingTracks(false);
      if (!paths.length) {
        setError(`No audio files found in "${folderName}".`);
        return;
      }
      beginPlaylistImport(info.id);
      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);
      const stopProgress = await listenToSyncProgress((p) => {
        if (typeof p.processed === "number") setImportedCount(p.processed);
        else if (typeof p.extracted === "number") setImportedCount(p.extracted);
        else if (typeof p.added === "number") setImportedCount(p.added);
      }).catch(() => null);
      try {
        await syncPlaylistFolder(info.id, paths);
        await loadPlaylists();
        await new Promise((r) => setTimeout(r, 0));
        await loadPlaylistTracks(info.id);
      } finally {
        stopProgress?.();
        endPlaylistImport(info.id);
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
      }
    } catch (err) {
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      setError(formatInvokeError(err, "Failed to add folder as playlist"));
      setIsAddingTracks(false);
      endPlaylistImport();
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
    }
  };

  const handleSyncPlaylistFolder = async (playlistId: string) => {
    // Look the folder up by the passed playlist id (not the currently viewed
    // playlist) so syncing works from lists like the mobile Settings page,
    // where the tapped playlist may not be the one currently open.
    const target = playlists.find((p) => p.id === playlistId);
    if (!target?.sync_folder) return;
    const firstTimeFill = target.track_count === 0;
    try {
      setError(null);
      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);
      if (firstTimeFill) beginPlaylistImport(playlistId);
      const stopProgress = await listenToSyncProgress((p) => {
        if (typeof p.processed === "number") setImportedCount(p.processed);
        else if (typeof p.extracted === "number") setImportedCount(p.extracted);
        else if (typeof p.added === "number") setImportedCount(p.added);
      }).catch(() => null);
      const folder = target.sync_folder;
      // Android SAF folders need a JS-side recursive scan; desktop Rust walks the path.
      const paths = androidHost ? await scanDirectoryRecursive(folder) : null;
      try {
        await syncPlaylistFolder(playlistId, paths);
        if (selectedPlaylistIdRef.current === playlistId) {
          await loadPlaylistTracks(playlistId);
        }
        await loadPlaylists();
      } finally {
        stopProgress?.();
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to sync folder"));
    } finally {
      if (firstTimeFill) endPlaylistImport(playlistId);
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
    }
  };

  const handleRemoveFromLibrary = async (path: string) => {
    try {
      setError(null);
      await removeTrackFromLibrary(path);
      if (playbackState.current_path === path) {
        await stopTrack();
        setSeekValue(0);
      }
      if (selectedPlaylistId) {
        await loadPlaylistTracks(selectedPlaylistId);
      }
      await loadPlaylists();
      await loadFavoritePaths();
      await loadQueueTracks();
      await updatePlaybackState();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to remove from library"));
    }
  };

  const handleRemoveFromPlaylist = async (path: string) => {
    try {
      setError(null);
      if (!selectedPlaylistId) return;
      if (isLibraryPlaylistName(selectedPlaylist?.name)) {
        await handleRemoveFromLibrary(path);
        return;
      }
      await removeTrackFromPlaylistById(selectedPlaylistId, path);
      await loadPlaylistTracks(selectedPlaylistId);
      await loadPlaylists();
      await loadFavoritePaths();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to remove from playlist"));
    }
  };

  return {
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
    runFolderImport,
    handleRemoveFromLibrary,
    handleRemoveFromPlaylist,
    setImportedCount,
  };
}
