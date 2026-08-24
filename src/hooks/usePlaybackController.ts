import { useEffect, useMemo, useRef, useState } from "react";
import {
  addToQueue,
  clearQueue,
  getPlaybackMode,
  getPlaybackState,
  getQueueTracks,
  moveQueueTrack,
  pauseTrack,
  playNext,
  playPrevious,
  playTrack,
  playTrackFromQueue,
  playTrackFromSpecificPlaylist,
  queueInsertNext,
  removeFromQueue,
  resumeTrack,
  seekTrack,
  setPlayerVolume,
  setRepeat,
  setShuffle,
  stopTrack,
  toggleFavorite,
  updateMediaPosition,
  type PlaybackMode,
  type PlaybackState,
  type PlaylistInfo,
  type QueueTrackState,
  type Track,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";
import { emptyPlaybackState } from "../utils/track";
import type { ContextMenuAnchor } from "../components/ContextMenu";
import type { SortDirection } from "../components/LibraryTrackList";

/** Playback transport (play/pause/seek/volume/shuffle/repeat), the in-memory
 * playback queue, and favorites. Owns `currentTrack` and the derived
 * position/duration values other panels (lyrics, player bar) read. */
export function usePlaybackController({
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
  onNoTrackFallback,
}: {
  androidHost: boolean;
  androidHostRef: { current: boolean };
  playlist: Track[];
  playlists: PlaylistInfo[];
  sortedPlaylist: Track[];
  sortDirection: SortDirection;
  selectedPlaylistId: string | null;
  loadPlaylists: () => Promise<PlaylistInfo[]>;
  loadPlaylistTracks: (playlistId: string) => Promise<boolean>;
  favoritePaths: Set<string>;
  setFavoritePaths: (updater: Set<string> | ((prev: Set<string>) => Set<string>)) => void;
  setError: (message: string | null) => void;
  setMenuTrackPath: (path: string | null) => void;
  setAddToPlaylistTrack: (path: string | null) => void;
  onNoTrackFallback: () => void;
}) {
  const [playbackState, setPlaybackState] = useState<PlaybackState>(emptyPlaybackState);
  const [seekValue, setSeekValue] = useState(0);
  const [volumeValue, setVolumeValue] = useState(0.8);
  const [playbackMode, setPlaybackMode] = useState<PlaybackMode>({
    repeat: "off",
    shuffle: false,
  });
  const [queueData, setQueueData] = useState<QueueTrackState>({
    tracks: [],
    current_index: null,
    is_shuffled: false,
  });
  const [showQueue, setShowQueue] = useState(false);
  const [queueMenuIndex, setQueueMenuIndex] = useState<number | null>(null);
  const [queueMenuAnchor, setQueueMenuAnchor] = useState<ContextMenuAnchor | null>(null);

  // Android uses system volume — keep Wave at 100% always.
  useEffect(() => {
    if (!androidHost) return;
    setVolumeValue(1);
    void setPlayerVolume(1);
  }, [androidHost]);

  const currentTrack = useMemo(() => {
    if (!playbackState.current_path) return null;
    const fromQueue = queueData.tracks.find(
      (track) => track.path === playbackState.current_path,
    );
    if (fromQueue) return fromQueue;
    const fromPlaylist = playlist.find((track) => track.path === playbackState.current_path);
    return fromPlaylist ?? null;
  }, [playbackState.current_path, queueData.tracks, playlist]);

  const hasActiveQueue = queueData.tracks.length > 0;
  const canSkip = hasActiveQueue || playlist.length > 0;
  const displayDuration =
    playbackState.duration_seconds ?? currentTrack?.duration_seconds ?? 0;
  const displayPosition = Math.min(seekValue, displayDuration || seekValue);

  const updatePlaybackState = async () => {
    const state = await getPlaybackState();
    setPlaybackState({ ...emptyPlaybackState, ...state });
    if (androidHostRef.current) {
      if ((state.volume ?? 1) !== 1) {
        void setPlayerVolume(1);
      }
      setVolumeValue(1);
    } else {
      setVolumeValue(state.volume ?? 0.8);
    }
    if (!document.body.classList.contains("is-seeking")) {
      setSeekValue(state.position_seconds ?? 0);
    }
    // Keep the OS media controls position in sync during playback and pause.
    // Never clear the session just because a poll saw no path — that tears down
    // the live notification / FGS and causes flicker + empty "wave" cards during
    // startup and folder sync. Backend stop/auto-advance already call clear.
    if (state.current_path) {
      updateMediaPosition(state.position_seconds, state.is_playing).catch(console.error);
    }
  };

  const loadPlaybackMode = async () => {
    try {
      const mode = await getPlaybackMode();
      setPlaybackMode(mode);
    } catch {
      /* ignore */
    }
  };

  const loadQueueTracks = async () => {
    const data = await getQueueTracks();
    setQueueData(data);
  };

  const handlePlayTrack = async (sortedIndex: number) => {
    try {
      setError(null);
      if (!selectedPlaylistId) return;
      const sortedPaths = sortedPlaylist.map((t) => t.path);
      await playTrackFromSpecificPlaylist(
        selectedPlaylistId,
        sortedIndex,
        sortDirection !== "none" ? sortedPaths : undefined,
      );
      await updatePlaybackState();
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Could not start playback"));
    }
  };

  const handlePlayPause = async () => {
    try {
      setError(null);
      if (playbackState.is_playing) {
        await pauseTrack();
      } else if (playbackState.is_paused && playbackState.current_path) {
        // Resume whatever is loaded — do not gate on playlist membership
        // (materialized Android paths often differ from library URIs).
        await resumeTrack();
      } else if (playbackState.current_path) {
        try {
          await playTrack(playbackState.current_path);
        } catch {
          // File may be gone; fall through to queue / playlist / picker.
          if (hasActiveQueue && queueData.current_index != null) {
            await playTrackFromQueue(queueData.current_index);
          } else if (hasActiveQueue) {
            await playTrackFromQueue(0);
          } else if (playlist.length > 0 && selectedPlaylistId) {
            await playTrackFromSpecificPlaylist(selectedPlaylistId, 0);
          } else {
            onNoTrackFallback();
            return;
          }
        }
      } else if (hasActiveQueue && queueData.current_index != null) {
        await playTrackFromQueue(queueData.current_index);
      } else if (hasActiveQueue) {
        await playTrackFromQueue(0);
      } else if (playlist.length > 0 && selectedPlaylistId) {
        await playTrackFromSpecificPlaylist(selectedPlaylistId, 0);
      } else {
        // Absolutely nothing loaded — invite the user to add music.
        onNoTrackFallback();
        return;
      }
      await updatePlaybackState();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to control playback"));
    }
  };

  const handleStop = async () => {
    try {
      setError(null);
      await stopTrack();
      setSeekValue(0);
      await updatePlaybackState();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to stop track"));
    }
  };

  const handlePrevious = async () => {
    if (!canSkip) return;
    try {
      setError(null);
      const path = await playPrevious();
      if (!path && selectedPlaylistId && sortedPlaylist.length > 0) {
        const orderedPaths = sortedPlaylist.map((t) => t.path);
        const fromIndex = sortedPlaylist.findIndex(
          (track) => track.path === playbackState.current_path,
        );
        const prevIndex =
          fromIndex > 0
            ? fromIndex - 1
            : fromIndex === 0
              ? sortedPlaylist.length - 1
              : Math.max(0, sortedPlaylist.length - 1);
        await playTrackFromSpecificPlaylist(
          selectedPlaylistId,
          prevIndex,
          sortDirection !== "none" ? orderedPaths : undefined,
        );
      }
      await updatePlaybackState();
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to go to previous track"));
    }
  };

  const handleNext = async () => {
    if (!canSkip) return;
    try {
      setError(null);
      const path = await playNext();
      if (!path && selectedPlaylistId && sortedPlaylist.length > 0) {
        const orderedPaths = sortedPlaylist.map((t) => t.path);
        const fromIndex = sortedPlaylist.findIndex(
          (track) => track.path === playbackState.current_path,
        );
        const nextIndex = fromIndex >= 0 ? (fromIndex + 1) % sortedPlaylist.length : 0;
        await playTrackFromSpecificPlaylist(
          selectedPlaylistId,
          nextIndex,
          sortDirection !== "none" ? orderedPaths : undefined,
        );
      }
      await updatePlaybackState();
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to go to next track"));
    }
  };

  const handleSeek = async (value: number) => {
    try {
      setSeekValue(value);
      if (playbackState.current_path) {
        await seekTrack(value);
        await updatePlaybackState();
      }
    } catch (err) {
      setError(formatInvokeError(err, "Failed to seek track"));
    } finally {
      document.body.classList.remove("is-seeking");
    }
  };

  const handlePlayPauseRef = useRef(handlePlayPause);
  const handleSeekRef = useRef(handleSeek);
  const mediaKeysRef = useRef({ position: 0, duration: 0, hasTrack: false });
  handlePlayPauseRef.current = handlePlayPause;
  handleSeekRef.current = handleSeek;
  mediaKeysRef.current = {
    position: displayPosition,
    duration: displayDuration,
    hasTrack: Boolean(playbackState.current_path),
  };

  useEffect(() => {
    const isEditableTarget = (target: EventTarget | null) => {
      if (!(target instanceof HTMLElement)) return false;
      const tag = target.tagName;
      return (
        tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable
      );
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target))
        return;

      if (event.code === "Space" || event.key === " ") {
        event.preventDefault();
        void handlePlayPauseRef.current();
        return;
      }

      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        const { position, duration, hasTrack } = mediaKeysRef.current;
        if (!hasTrack) return;
        event.preventDefault();
        const delta = event.key === "ArrowLeft" ? -5 : 5;
        const next = Math.max(0, position + delta);
        void handleSeekRef.current(duration > 0 ? Math.min(duration, next) : next);
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const handleVolume = async (value: number) => {
    if (androidHostRef.current) {
      value = 1;
    }
    try {
      setVolumeValue(value);
      await setPlayerVolume(value);
      await updatePlaybackState();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to set volume"));
    }
  };

  const handleToggleFavorite = async (path: string) => {
    // Optimistic update: flip the heart immediately so the UI feels instant.
    const wasFavorited = favoritePaths.has(path);
    setFavoritePaths((prev) => {
      const next = new Set(prev);
      if (wasFavorited) next.delete(path);
      else next.add(path);
      return next;
    });
    try {
      setError(null);
      await toggleFavorite(path);
      // Refresh playlist counts in the background (don't block the heart UI).
      loadPlaylists().catch(() => {});
      // If viewing the Favorites playlist, refresh its tracks so it stays accurate.
      const favPlaylist = playlists.find((p) => p.name === "Favorites");
      if (favPlaylist && selectedPlaylistId === favPlaylist.id) {
        await loadPlaylistTracks(favPlaylist.id);
      }
    } catch (err) {
      // Revert on failure.
      setFavoritePaths((prev) => {
        const next = new Set(prev);
        if (wasFavorited) next.add(path);
        else next.delete(path);
        return next;
      });
      setError(formatInvokeError(err, "Failed to toggle favorite"));
    }
  };

  const handlePlayNext = async (path: string) => {
    try {
      setError(null);
      await queueInsertNext(path);
      setMenuTrackPath(null);
      if (!playbackState.current_path && !playbackState.is_paused) {
        const data = await getQueueTracks();
        const idx = data.tracks.findIndex((track) => track.path === path);
        if (idx >= 0) {
          await playTrackFromQueue(idx);
          await updatePlaybackState();
        }
      }
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to add track to play next"));
    }
  };

  const handleAddToQueue = async (path: string) => {
    try {
      setError(null);
      await addToQueue(path);
      setMenuTrackPath(null);
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to add track to queue"));
    }
  };

  const handleRemoveFromQueue = async (index: number) => {
    try {
      setError(null);
      await removeFromQueue(index);
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to remove from queue"));
    }
  };

  const handleMoveQueueTrack = async (from: number, to: number) => {
    if (to < 0 || to >= queueData.tracks.length) return;
    try {
      setError(null);
      setQueueMenuIndex(null);
      setQueueMenuAnchor(null);
      await moveQueueTrack(from, to);
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to reorder queue"));
    }
  };

  const closeQueueContextMenu = () => {
    setQueueMenuIndex(null);
    setQueueMenuAnchor(null);
  };

  const openQueueContextMenu = (index: number, anchor: ContextMenuAnchor) => {
    setMenuTrackPath(null);
    setAddToPlaylistTrack(null);
    setQueueMenuIndex(index);
    setQueueMenuAnchor(anchor);
  };

  const handleClearQueue = async () => {
    try {
      setError(null);
      await clearQueue();
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to clear queue"));
    }
  };

  const handleToggleShuffle = async () => {
    try {
      const next = !playbackMode.shuffle;
      await setShuffle(next);
      await loadPlaybackMode();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to toggle shuffle"));
    }
  };

  const handleCycleRepeat = async () => {
    try {
      const next =
        playbackMode.repeat === "off" ? "all" : playbackMode.repeat === "all" ? "one" : "off";
      await setRepeat(next);
      await loadPlaybackMode();
    } catch (err) {
      setError(formatInvokeError(err, "Failed to change repeat mode"));
    }
  };

  const handlePlayFromQueue = async (index: number) => {
    try {
      setError(null);
      await playTrackFromQueue(index);
      await updatePlaybackState();
      await loadQueueTracks();
    } catch (err) {
      setError(formatInvokeError(err, "Could not start playback"));
    }
  };

  return {
    playbackState,
    setPlaybackState,
    seekValue,
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
    hasActiveQueue,
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
  };
}
