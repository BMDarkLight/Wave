import { useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import {
  clearPlaylistById,
  deletePlaylist,
  exportPlaylist,
  getFileName,
  getPlaylistTracksById,
  importPlaylist,
  listPlaylists,
  openPlaylistDialog,
  savePlaylistDialog,
  selectAudioFolder,
  selectMediaFolder,
  type PlaylistInfo,
  type Track,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";
import { LIBRARY_PLAYLIST_NAME, isLibraryPlaylistName } from "../utils/track";
import type { MainView } from "../components/Sidebar";
import type { PlaylistDialogState } from "../components/dialogs/CreatePlaylistDialog";

type BrowsePage =
  | { kind: "artist"; name: string }
  | { kind: "album"; name: string; albumArtist: string | null };

/** Playlist CRUD (create/rename/delete/clear/export/import), the create/rename
 * dialog, and the album/artist browse stack. Navigation-level orchestration
 * (select-playlist, go-home, etc.) that also touches search/mobile-nav/media
 * import state stays in App.tsx. */
export function usePlaylistManager({
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
}: {
  playlists: PlaylistInfo[];
  setPlaylists: Dispatch<SetStateAction<PlaylistInfo[]>>;
  selectedPlaylistId: string | null;
  androidHost: boolean;
  setError: (message: string | null) => void;
  setPlaylist: Dispatch<SetStateAction<Track[]>>;
  setIsLoadingPlaylist: (loading: boolean) => void;
  setIsLoading: (loading: boolean) => void;
  loadFavoritePaths: () => Promise<void>;
  setMobileNavOpen: (open: boolean) => void;
  setMainView: (view: MainView) => void;
  setActivePlaylistId: (id: string | null) => void;
  selectedPlaylistIdRef: { current: string | null };
}) {
  // Album / artist browse stack (artist → album nests correctly for back)
  const [browseStack, setBrowseStack] = useState<BrowsePage[]>([]);
  const browseTop = browseStack[browseStack.length - 1] ?? null;
  const viewingAlbum =
    browseTop?.kind === "album"
      ? { name: browseTop.name, albumArtist: browseTop.albumArtist }
      : null;
  const viewingArtist = browseTop?.kind === "artist" ? browseTop.name : null;

  const openArtistPage = (name: string) => {
    setBrowseStack([{ kind: "artist", name }]);
  };
  const openAlbumPage = (name: string, albumArtist: string | null) => {
    setBrowseStack([{ kind: "album", name, albumArtist }]);
  };
  const pushAlbumPage = (name: string, albumArtist: string | null) => {
    setBrowseStack((stack) => [...stack, { kind: "album", name, albumArtist }]);
  };
  const browseBack = () => {
    setBrowseStack((stack) => stack.slice(0, -1));
  };
  const clearBrowse = () => {
    setBrowseStack([]);
  };

  const [showClearConfirm, setShowClearConfirm] = useState(false);
  const [deletePlaylistConfirm, setDeletePlaylistConfirm] = useState<{
    id: string;
    name: string;
  } | null>(null);

  const [playlistDialog, setPlaylistDialog] = useState<PlaylistDialogState | null>(null);
  const [playlistNameInput, setPlaylistNameInput] = useState("");
  const [playlistSyncFolder, setPlaylistSyncFolderInput] = useState<string | null>(null);
  const [playlistDialogError, setPlaylistDialogError] = useState<string | null>(null);
  const playlistNameInputRef = useRef<HTMLInputElement>(null);
  const playlistLoadSeqRef = useRef(0);

  const selectedPlaylist = playlists.find((p) => p.id === selectedPlaylistId) ?? null;

  const sortedPlaylists = useMemo(() => {
    const priority = [LIBRARY_PLAYLIST_NAME, "Favorites"];
    return [...playlists].sort((a, b) => {
      const ai = priority.indexOf(a.name);
      const bi = priority.indexOf(b.name);
      return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
    });
  }, [playlists]);

  const libraryPlaylist = useMemo(
    () => playlists.find((p) => isLibraryPlaylistName(p.name)) ?? null,
    [playlists],
  );
  const favoritesPlaylist = useMemo(
    () => playlists.find((p) => p.name === "Favorites") ?? null,
    [playlists],
  );
  const userPlaylists = useMemo(
    () => sortedPlaylists.filter((p) => !isLibraryPlaylistName(p.name) && p.name !== "Favorites"),
    [sortedPlaylists],
  );

  const loadPlaylists = async () => {
    const list = await listPlaylists();
    setPlaylists(list);
    return list;
  };

  const loadPlaylistTracks = async (playlistId: string) => {
    const seq = ++playlistLoadSeqRef.current;
    const tracks = await getPlaylistTracksById(playlistId);
    // Ignore stale responses from a prior playlist selection.
    if (seq !== playlistLoadSeqRef.current || selectedPlaylistIdRef.current !== playlistId) {
      return false;
    }
    setPlaylist(tracks);
    await loadFavoritePaths();
    return true;
  };

  // Resolve the default playlist ID from the playlists list.
  const getDefaultPlaylistId = (list: PlaylistInfo[]): string | null => {
    return (list.find((p) => isLibraryPlaylistName(p.name)) ?? list[0])?.id ?? null;
  };

  const handleClearPlaylist = async () => {
    if (selectedPlaylist?.sync_folder) {
      setError("Synced playlists cannot be cleared.");
      return;
    }
    setShowClearConfirm(true);
  };

  const confirmClearPlaylist = async () => {
    try {
      setError(null);
      setIsLoading(true);
      setShowClearConfirm(false);
      if (!selectedPlaylistId) return;
      await clearPlaylistById(selectedPlaylistId);
      await loadPlaylistTracks(selectedPlaylistId);
      await loadPlaylists();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to clear playlist"));
    } finally {
      setIsLoading(false);
    }
  };

  const openCreatePlaylistDialog = () => {
    setMobileNavOpen(false);
    setPlaylistNameInput("");
    setPlaylistSyncFolderInput(null);
    setPlaylistDialogError(null);
    setPlaylistDialog({ mode: "create" });
  };

  const openRenamePlaylistDialog = (playlistId: string, currentName: string) => {
    setMobileNavOpen(false);
    setPlaylistNameInput(currentName);
    setPlaylistSyncFolderInput(null);
    setPlaylistDialogError(null);
    setPlaylistDialog({ mode: "rename", playlistId, currentName });
  };

  const closePlaylistDialog = () => {
    setPlaylistDialog(null);
    setPlaylistSyncFolderInput(null);
    setPlaylistDialogError(null);
  };

  const pickPlaylistSyncFolder = async () => {
    try {
      if (androidHost) {
        const result = await selectMediaFolder();
        if (!result) return;
        setPlaylistSyncFolderInput(result.uri);
        if (!playlistNameInput.trim()) {
          setPlaylistNameInput(result.displayName || getFileName(result.uri));
        }
        return;
      }
      const directory = await selectAudioFolder();
      if (!directory) return;
      setPlaylistSyncFolderInput(directory);
      if (!playlistNameInput.trim()) {
        setPlaylistNameInput(getFileName(directory));
      }
    } catch (err) {
      setPlaylistDialogError(formatInvokeError(err, "Failed to select folder"));
    }
  };

  const handleDeletePlaylist = async (id: string) => {
    const playlistInfo = playlists.find((p) => p.id === id);
    setDeletePlaylistConfirm({ id, name: playlistInfo?.name ?? "Unknown" });
  };

  const confirmDeletePlaylist = async () => {
    if (!deletePlaylistConfirm) return;
    const { id } = deletePlaylistConfirm;
    setDeletePlaylistConfirm(null);
    try {
      setError(null);
      await deletePlaylist(id);
      const list = await loadPlaylists();
      if (selectedPlaylistId === id) {
        const defaultId = getDefaultPlaylistId(list);
        if (defaultId) {
          setActivePlaylistId(defaultId);
          setPlaylist([]);
          setIsLoadingPlaylist(true);
          try {
            await loadPlaylistTracks(defaultId);
          } finally {
            if (selectedPlaylistIdRef.current === defaultId) {
              setIsLoadingPlaylist(false);
            }
          }
        }
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to delete playlist"));
    }
  };

  const handleExportPlaylistById = async (playlistId: string, playlistName: string) => {
    try {
      setError(null);
      const path = await savePlaylistDialog(playlistName);
      if (!path) return;
      const exportFormat = path.toLowerCase().endsWith(".json") ? "json" : "m3u";
      await exportPlaylist(playlistId, path, exportFormat);
    } catch (err) {
      setError(formatInvokeError(err, `Failed to export "${playlistName}"`));
    }
  };

  const handleImportPlaylist = async () => {
    try {
      setError(null);
      const path = await openPlaylistDialog();
      if (!path) return;
      const result = await importPlaylist(path);
      await loadPlaylists();
      setActivePlaylistId(result.playlist_id);
      setMainView("playlist");
      await loadPlaylistTracks(result.playlist_id);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to import playlist"));
    }
  };

  return {
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
    sortedPlaylists,
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
  };
}
