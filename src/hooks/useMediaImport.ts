/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useRef, useState } from "react";
import {
  addTrackToPlaylistById,
  addTracksToPlaylistById,
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
  selectAudioFile,
  selectAudioFolder,
  selectMediaFolder,
  setOutputDevice,
  setPlaylistSyncFolder,
  stopTrack,
  syncPlaylistFolder,
  type PlaybackState,
  type PlaylistInfo,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";
import { isLibraryPlaylistName } from "../utils/track";

/** Where the import started from the Welcome dialog stands, for the progress
 *  dialog that follows it. */
export interface ImportProgress {
  phase: "finding" | "adding" | "done";
  /** Songs read so far, out of `total`. */
  processed: number;
  /** New songs to add; 0 until the folder has been listed. */
  total: number;
  added: number;
}

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
  const [importingPlaylistId, setImportingPlaylistId] = useState<string | null>(
    null,
  );
  const importingPlaylistIdRef = useRef<string | null>(null);
  const isImporting = importingPlaylistId != null;
  const [importedCount, setImportedCount] = useState(0);
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(
    null,
  );

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

  /** Playlist the "add from library" picker is adding to, while it is open. */
  const [addFromLibraryTarget, setAddFromLibraryTarget] = useState<
    string | null
  >(null);
  const showAddFromLibrary = addFromLibraryTarget != null;
  const [librarySearchAdding, setLibrarySearchAdding] = useState(false);

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

  /** Open the picker for a playlist, the one on screen unless told otherwise
   * (a playlist that was only just created isn't selected yet). */
  const openAddFromLibrary = (playlistId?: string) => {
    const target = playlistId ?? selectedPlaylistId;
    if (!target) return;
    setLibrarySearchAdding(false);
    setAddFromLibraryTarget(target);
  };

  const closeAddFromLibrary = () => {
    setAddFromLibraryTarget(null);
    setLibrarySearchAdding(false);
  };

  const addPathsFromLibraryModal = async (paths: string[]) => {
    const playlistId = addFromLibraryTarget;
    if (!playlistId || paths.length === 0) return;
    setLibrarySearchAdding(true);
    setError(null);
    try {
      const added = await addTracksToPlaylistById(playlistId, paths);
      if (added < paths.length) {
        const missed = paths.length - added;
        setError(
          `${missed} song${missed === 1 ? " was" : "s were"} already there or could not be read.`,
        );
      }
      closeAddFromLibrary();
      await loadPlaylistTracks(playlistId);
      await loadPlaylists();
    } catch (err) {
      setLibrarySearchAdding(false);
      setError(formatInvokeError(err, "Failed to add songs"));
    }
  };

  const handleAddSelectedFromLibrary = (paths: string[]) =>
    addPathsFromLibraryModal(paths);

  const handlePickFileFromLibraryModal = async () => {
    try {
      setError(null);
      const paths = await selectAudioFile(true);
      if (!paths?.length) return;
      await addPathsFromLibraryModal(paths);
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
        setError(
          `Finished importing folder with ${result.errors.length} failure(s).`,
        );
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

  /** `showProgress` opens the import progress dialog once a folder is
   *  picked. The Welcome dialog uses it; the import runs the same either way. */
  /** Counts and the progress screen follow the backend's sync events. */
  const followImport = (
    p: Parameters<Parameters<typeof listenToSyncProgress>[0]>[0],
  ) => {
    if (typeof p.processed === "number") setImportedCount(p.processed);
    else if (typeof p.extracted === "number") setImportedCount(p.extracted);
    else if (typeof p.added === "number") setImportedCount(p.added);
    if (p.phase === "diff" || p.phase === "extract") {
      setImportProgress(
        (current) =>
          current && {
            ...current,
            phase: "adding",
            total: p.to_add ?? current.total,
            processed: p.processed ?? current.processed,
          },
      );
    }
  };

  const finishImport = (added: number) =>
    setImportProgress(
      (current) =>
        current && {
          ...current,
          phase: "done",
          processed: current.total,
          added,
        },
    );

  const handleAddFolderAndroid = async ({
    showProgress = false,
  }: { showProgress?: boolean } = {}) => {
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
      if (showProgress) {
        setImportProgress({
          phase: "finding",
          processed: 0,
          total: 0,
          added: 0,
        });
      }

      const directory = result.uri;

      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);

      const paths = await scanDirectoryRecursive(directory);
      if (!paths.length) {
        setImportProgress(null);
        setError("No audio files found in the selected folder.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }

      // Android media scan always targets Library.
      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const playlistId =
        list.find((p) => isLibraryPlaylistName(p.name))?.id ??
        getDefaultPlaylistId(list);
      if (!playlistId) {
        setImportProgress(null);
        setError("No playlist selected.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }

      await setPlaylistSyncFolder(playlistId, directory);
      await saveMediaFolder(directory).catch(() => {});

      beginPlaylistImport(playlistId);
      const stopProgress = await listenToSyncProgress(followImport).catch(
        () => null,
      );

      try {
        const syncResult = await syncPlaylistFolder(playlistId, paths);
        finishImport(syncResult.added);
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
      setImportProgress(null);
      endPlaylistImport();
      setIsScanningFolder(false);
      setFolderScanIsSync(false);
      setError(formatInvokeError(err, "Failed to scan folder"));
    }
  };

  /** Mobile Settings "Media Source Folders" — Android uses the SAF folder
   *  picker; a desktop window narrowed into the mobile layout falls back to
   *  the regular directory picker, mirroring handleAddFolderAndroid. */
  const handleAddMediaSource = async ({
    showProgress = false,
  }: { showProgress?: boolean } = {}) => {
    if (androidHost) {
      await handleAddFolderAndroid({ showProgress });
      return;
    }
    try {
      setError(null);
      const directory = await selectAudioFolder();
      if (!directory) return;
      if (showProgress) {
        setImportProgress({
          phase: "finding",
          processed: 0,
          total: 0,
          added: 0,
        });
      }
      setIsScanningFolder(true);
      setFolderScanIsSync(true);
      setImportedCount(0);
      const paths = await scanDirectory(directory);
      if (!paths.length) {
        setImportProgress(null);
        setError("No audio files found in the selected folder.");
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }
      const list = playlists.length > 0 ? playlists : await loadPlaylists();
      const playlistId =
        list.find((p) => isLibraryPlaylistName(p.name))?.id ??
        getDefaultPlaylistId(list);
      if (!playlistId) {
        setImportProgress(null);
        setIsScanningFolder(false);
        setFolderScanIsSync(false);
        return;
      }
      await setPlaylistSyncFolder(playlistId, directory);
      await saveMediaFolder(directory).catch(() => {});
      beginPlaylistImport(playlistId);
      const stopProgress = await listenToSyncProgress(followImport).catch(
        () => null,
      );
      try {
        const synced = await syncPlaylistFolder(playlistId, paths);
        finishImport(synced.added);
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
      setImportProgress(null);
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
        list.find((p) => isLibraryPlaylistName(p.name))?.id ??
        getDefaultPlaylistId(list);
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
    addFromLibraryTarget,
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
    handleAddSelectedFromLibrary,
    handlePickFileFromLibraryModal,
    handleAddFolder,
    handleAddFolderAndroid,
    importProgress,
    closeImportProgress: () => setImportProgress(null),
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
