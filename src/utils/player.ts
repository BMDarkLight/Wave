// Use dynamic imports to handle cases where Tauri API might not be available
let invokeFn: typeof import("@tauri-apps/api/core").invoke;
let openFn: typeof import("@tauri-apps/plugin-dialog").open;

const TAURI_UNAVAILABLE =
  "Tauri API is not available. Open Wave through the Tauri app (desktop or Android), not a plain browser tab.";

const initTauri = async () => {
  try {
    if (!("__TAURI_INTERNALS__" in window)) {
      console.warn(TAURI_UNAVAILABLE);
      return false;
    }

    const core = await import("@tauri-apps/api/core");
    const dialog = await import("@tauri-apps/plugin-dialog");
    invokeFn = core.invoke;
    openFn = dialog.open;
    console.log("Tauri APIs initialized");
    return true;
  } catch (error) {
    console.error("Failed to initialize Tauri APIs:", error);
    return false;
  }
};

let tauriInitialized = initTauri();

const safeInvoke = async <T = any>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
  await tauriInitialized;

  if (!invokeFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }

  return await invokeFn<T>(cmd, args);
};

export const safeInvokeHostOs = (): Promise<string> => safeInvoke<string>("host_os");

export interface ImportAudioResult {
  paths: string[];
  errors: string[];
}

export const importAudioSources = (paths: string[]): Promise<ImportAudioResult> => {
  return safeInvoke<ImportAudioResult>("import_audio_sources", { paths });
};

export interface PlaybackState {
  is_playing: boolean;
  is_paused: boolean;
  current_path: string | null;
  position_seconds: number;
  duration_seconds: number | null;
  volume: number;
  output_device_name: string;
}

export interface Track {
  id: string;
  path: string;
  name: string;
  title: string;
  artist: string;
  album: string;
  album_artist: string | null;
  genre: string | null;
  year: number | null;
  track_number: number | null;
  disc_number: number | null;
  format: string;
  duration_seconds: number | null;
  sample_rate: number | null;
  channels: number | null;
  bit_depth: number | null;
  lyrics: string | null;
  lyrics_source: string | null;
  /** Absolute thumb path, data URL, or https URL. */
  cover_art_data_url: string | null;
  cover_art_mime: string | null;
  cover_art_source: string | null;
  album_art_id?: string | null;
  fingerprint_sha256: string | null;
  acoustid_fingerprint: string | null;
  musicbrainz_recording_id: string | null;
  file_size: number;
  modified_at: number;
  indexed_at: number;
  is_saf_uri?: boolean;
}

export interface QueueState {
  tracks: string[];
  current_index: number | null;
  is_shuffled: boolean;
}

export interface PlaybackMode {
  repeat: "off" | "one" | "all";
  shuffle: boolean;
}

export interface PlaylistInfo {
  id: string;
  profile_id: string;
  name: string;
  track_count: number;
  created_at: number;
  updated_at: number;
  /** Folder path/URI this playlist is media-synced with, if any. */
  sync_folder?: string | null;
}

export interface QueueTrackState {
  tracks: Track[];
  current_index: number | null;
  is_shuffled: boolean;
}

export interface ImportResult {
  playlist_id: string;
  playlist_name: string;
  track_count: number;
}

/** Summary of a distinct album in the library (grouped by album + album artist). */
export interface AlbumSummary {
  name: string;
  album_artist: string | null; // resolved: tag album_artist, else track artist
  artist: string;              // representative track artist
  track_count: number;
  year: number | null;
  cover_art_data_url: string | null; // thumb path for list/grid fallback
  cover_art_mime: string | null;
  /** Track path used to extract a full-resolution embedded cover. */
  cover_track_path: string | null;
}

/** Summary of a distinct artist in the library, with aggregate counts. */
export interface ArtistSummary {
  name: string;
  track_count: number;
  album_count: number;
}

export const playTrack = (path: string): Promise<void> => {
  return safeInvoke("play_track", { path });
};

/** Replace the queue with `paths` and start playback at `index`. */
export const playTracks = (paths: string[], index: number): Promise<void> => {
  return safeInvoke("play_tracks", { paths, index });
};

export const pauseTrack = (): Promise<void> => {
  return safeInvoke("pause_track");
};

export const resumeTrack = (): Promise<void> => {
  return safeInvoke("resume_track");
};

export const stopTrack = (): Promise<void> => {
  return safeInvoke("stop_track");
};

export const seekTrack = (seconds: number): Promise<void> => {
  return safeInvoke("seek_track", { seconds });
};

export const setPlayerVolume = (volume: number): Promise<void> => {
  return safeInvoke("set_volume", { volume });
};

export const getPlaybackState = (): Promise<PlaybackState> => {
  return safeInvoke<PlaybackState>("get_playback_state");
};

export const selectAudioFile = async (multiple: boolean = false): Promise<string[] | null> => {
  await tauriInitialized;

  if (!openFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }

  // Android ignores file extensions and expects MIME types in `extensions`.
  // Opening with only `.mp3`-style filters can show an empty/broken picker.
  const { isAndroid } = await import("./platform");
  const android = await isAndroid();

  const selected = await openFn({
    multiple,
    directory: false,
    filters: android
      ? [
          {
            name: "Audio",
            extensions: [
              "audio/*",
              "audio/mpeg",
              "audio/mp4",
              "audio/aac",
              "audio/flac",
              "audio/ogg",
              "audio/wav",
              "audio/x-wav",
              "audio/opus",
              "application/ogg",
            ],
          },
        ]
      : [
          {
            name: "Audio",
            extensions: [
              "aac", "aiff", "alac", "caf", "flac", "m4a", "m4b", "m4p", "mka", "mkv",
              "mp1", "mp2", "mp3", "mp4", "oga", "ogg", "opus", "wav", "wave", "weba",
            ],
          },
        ],
    title: multiple ? "Select Audio Files" : "Select Audio File",
  });

  if (selected === null) return null;
  if (Array.isArray(selected)) return selected;
  if (typeof selected === "string") return [selected];
  return null;
};

export const selectAudioFolder = async (): Promise<string | null> => {
  await tauriInitialized;

  if (!openFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }

  const selected = await openFn({
    directory: true,
    title: "Select Music Folder",
  });

  if (selected === null) return null;
  if (typeof selected === "string") return selected;
  return null;
};

function invokeErrorMessage(err: unknown, fallback: string): string {
  if (typeof err === "string" && err.trim()) return err;
  if (err instanceof Error && err.message.trim()) return err.message;
  if (err && typeof err === "object") {
    const obj = err as Record<string, unknown>;
    for (const key of ["message", "error", "data"] as const) {
      const value = obj[key];
      if (typeof value === "string" && value.trim()) return value;
      if (value && typeof value === "object" && "message" in (value as object)) {
        const nested = (value as { message?: unknown }).message;
        if (typeof nested === "string" && nested.trim()) return nested;
      }
    }
  }
  return fallback;
}

export const selectMediaFolder = async (): Promise<{ uri: string; displayName?: string } | null> => {
  await tauriInitialized;

  if (!invokeFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }

  try {
    const result = await invokeFn<{ uri: string; display_name?: string }>("pick_media_folder");
    return { uri: result.uri, displayName: result.display_name };
  } catch (err) {
    const message = invokeErrorMessage(err, "Failed to pick media folder");
    // Cancel is not a failure — return null so callers can no-op quietly.
    if (/cancel/i.test(message)) {
      return null;
    }
    console.error("Failed to pick media folder:", err);
    throw new Error(message);
  }
};

export const getFileName = (path: string | null): string => {
  if (!path) return "No track selected";
  // content://.../document/primary:Music/song.mp3 or plain paths
  const cleaned = path.split("?")[0] ?? path;
  const parts = cleaned.split(/[/\\:]/);
  const last = parts[parts.length - 1] || "Unknown";
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
};

export const addTrackToPlaylist = (path: string): Promise<Track> => {
  return safeInvoke<Track>("add_track_to_playlist", { path });
};

export const removeTrackFromPlaylist = (path: string): Promise<void> => {
  return safeInvoke("remove_track_from_playlist", { path });
};

export const getPlaylist = (): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_playlist");
};

export const clearPlaylist = (): Promise<void> => {
  return safeInvoke("clear_playlist");
};

// ── Favorites ─────────────────────────────────────────────────────────────────
// "Favorites" is a special seeded playlist that appears in list_playlists and
// cannot be deleted or renamed. Use these helpers to manage it.

export const addTrackToFavorites = (path: string): Promise<Track> => {
  return safeInvoke<Track>("add_track_to_favorites", { path });
};

export const removeTrackFromFavorites = (path: string): Promise<void> => {
  return safeInvoke("remove_track_from_favorites", { path });
};

export const getFavorites = (): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_favorites");
};

export const isTrackInFavorites = (path: string): Promise<boolean> => {
  return safeInvoke<boolean>("is_track_in_favorites", { path });
};

export const isTrackInPlaylist = (path: string): Promise<boolean> => {
  return safeInvoke<boolean>("is_track_in_playlist", { path });
};

/** Toggle the favorite state of a track. Returns the new state (true = favorited). */
export const toggleFavorite = (path: string): Promise<boolean> => {
  return safeInvoke<boolean>("toggle_favorite", { path });
};

export const clearFavorites = (): Promise<void> => {
  return safeInvoke("clear_favorites");
};

export const playTrackFromPlaylist = (index: number): Promise<void> => {
  return safeInvoke("play_track_from_playlist", { index });
};

export const scanDirectory = (directory: string): Promise<string[]> => {
  return safeInvoke<string[]>("scan_directory", { directory });
};

export const indexMusicLibrary = (
  directory: string,
  profileId?: string,
  playlistName?: string
): Promise<Track[]> => {
  return safeInvoke<Track[]>("index_music_library", {
    directory,
    profileId,
    playlistName,
  });
};

export const listPlaylists = (profileId?: string): Promise<PlaylistInfo[]> => {
  return safeInvoke<PlaylistInfo[]>("list_playlists", { profileId });
};

export const getLibraryDatabasePath = (): Promise<string> => {
  return safeInvoke<string>("get_library_database_path");
};

export const getSupportedAudioExtensions = (): Promise<string[]> => {
  return safeInvoke<string[]>("get_supported_audio_extensions");
}

// ── Playlist CRUD ────────────────────────────────────────────────────────────

export const createPlaylist = (
  name: string,
  syncFolder?: string | null,
): Promise<PlaylistInfo> => {
  return safeInvoke<PlaylistInfo>("create_playlist", {
    name,
    syncFolder: syncFolder ?? null,
  });
};

export const setPlaylistSyncFolder = (
  id: string,
  syncFolder?: string | null,
): Promise<PlaylistInfo> => {
  return safeInvoke<PlaylistInfo>("set_playlist_sync_folder", {
    id,
    syncFolder: syncFolder ?? null,
  });
};

export const deletePlaylist = (id: string): Promise<void> => {
  return safeInvoke("delete_playlist", { id });
};

export const renamePlaylist = (id: string, name: string): Promise<void> => {
  return safeInvoke("rename_playlist", { id, name });
};

export const getPlaylistTracksById = (id: string): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_playlist_tracks_by_id", { id });
};

export const searchLibraryTracks = (
  query: string,
  limit?: number,
): Promise<Track[]> => {
  return safeInvoke<Track[]>("search_library_tracks", {
    query,
    limit: limit ?? null,
  });
};

export type SearchMatchField =
  | "title"
  | "artist"
  | "album"
  | "name"
  | "lyrics";

export interface SearchHit {
  track: Track;
  matched_fields: SearchMatchField[];
  lyrics_snippet: string | null;
}

/** Realtime search across title, artist, album, filename, and lyrics. */
export const searchLibrary = (
  query: string,
  limit?: number,
): Promise<SearchHit[]> => {
  return safeInvoke<SearchHit[]>("search_library", {
    query,
    limit: limit ?? null,
  });
};

export const addTrackToPlaylistById = (id: string, path: string): Promise<Track> => {
  return safeInvoke<Track>("add_track_to_playlist_by_id", { id, path });
};

export const clearAudioImports = (): Promise<number> => {
  return safeInvoke<number>("clear_audio_imports");
};

export type ResetAppResult = {
  tracks_removed: number;
  playlists_deleted: number;
};

/** Wipe library DB, media folder prefs, and cover caches. Keeps empty Library + Favorites. */
export const resetApp = (): Promise<ResetAppResult> => {
  return safeInvoke<ResetAppResult>("reset_app");
};

export const removeTrackFromPlaylistById = (id: string, path: string): Promise<void> => {
  return safeInvoke("remove_track_from_playlist_by_id", { id, path });
};

export const removeTrackFromLibrary = (path: string): Promise<void> => {
  return safeInvoke("remove_track_from_library", { path });
};

export const fetchLyricsForTrack = (path: string): Promise<Track | null> => {
  return safeInvoke<Track>("fetch_lyrics_for_track", { path }).catch(() => null);
};

/** Full track row including lyrics (list queries omit lyrics). */
export const getTrackDetails = (path: string): Promise<Track | null> => {
  return safeInvoke<Track | null>("get_track_details", { path }).catch(() => null);
};

/** Full embedded cover as a one-shot data URL (not stored). */
export const getTrackFullCover = (path: string): Promise<string | null> => {
  return safeInvoke<string | null>("get_track_full_cover", { path }).catch(() => null);
};

/** Resolve cover URL for <img src>. Absolute paths go through convertFileSrc. */
export const resolveCoverSrc = async (
  cover: string | null | undefined,
): Promise<string | null> => {
  if (!cover) return null;
  if (
    cover.startsWith("data:") ||
    cover.startsWith("http://") ||
    cover.startsWith("https://") ||
    cover.startsWith("asset:") ||
    cover.startsWith("blob:")
  ) {
    return cover;
  }

  // Strip file:// so convertFileSrc gets a real filesystem path.
  let path = cover;
  if (path.startsWith("file://")) {
    try {
      path = decodeURIComponent(path.replace(/^file:\/\//, ""));
    } catch {
      path = path.replace(/^file:\/\//, "");
    }
  }

  try {
    await tauriInitialized;
    const core = await import("@tauri-apps/api/core");
    return core.convertFileSrc(path);
  } catch {
    // Never return a bare path / file:// — WebView CSP will break the <img>.
    return null;
  }
};

export const clearPlaylistById = (id: string): Promise<void> => {
  return safeInvoke("clear_playlist_by_id", { id });
};

export const playTrackFromSpecificPlaylist = (
  playlistId: string,
  index: number,
  orderedPaths?: string[],
): Promise<void> => {
  return safeInvoke("play_track_from_specific_playlist", {
    playlistId,
    index,
    orderedPaths: orderedPaths ?? null,
  });
};

// ── Albums & artists (browse / query) ─────────────────────────────────────────

/** Create a playlist from every track matching an album name. */
export const createAlbumPlaylist = (album: string, name?: string): Promise<PlaylistInfo> => {
  return safeInvoke<PlaylistInfo>("create_album_playlist", { album, name });
};

/** Create a playlist from every track matching an artist name (a discography). */
export const createArtistPlaylist = (artist: string, name?: string): Promise<PlaylistInfo> => {
  return safeInvoke<PlaylistInfo>("create_artist_playlist", { artist, name });
};

/**
 * List every distinct album in the library, grouped by (album, album_artist).
 * Use for a Spotify-like album grid. `album_artist` is the resolved value
 * (tag `album_artist`, falling back to `artist`) — pass it back to
 * `getAlbumTracks` for a precise "go to album" result.
 */
export const listAlbums = (): Promise<AlbumSummary[]> => {
  return safeInvoke<AlbumSummary[]>("list_albums");
};

/** List every distinct artist with track and album counts. */
export const listArtists = (): Promise<ArtistSummary[]> => {
  return safeInvoke<ArtistSummary[]>("list_artists");
};

/**
 * Return every track in an album. Pass `albumArtist` (from an `AlbumSummary`
 * or a clicked `Track`'s `album_artist ?? artist`) to keep same-named albums by
 * different artists apart; omit it to match the album name only.
 */
export const getAlbumTracks = (album: string, albumArtist?: string | null): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_album_tracks", { album, albumArtist });
};

/** Return every track by an artist (a discography), ordered by album/disc/track. */
export const getArtistTracks = (artist: string): Promise<Track[]> => {
  return safeInvoke<Track[]>("get_artist_tracks", { artist });
};

/** Return distinct albums by an artist with aggregate info. */
export const getArtistAlbums = (artist: string): Promise<AlbumSummary[]> => {
  return safeInvoke<AlbumSummary[]>("get_artist_albums", { artist });
};

// ── Queue manipulation ──────────────────────────────────────────────────────

export const addToQueue = (path: string): Promise<void> => {
  return safeInvoke("add_to_queue", { path });
};

export const queueInsertNext = (path: string): Promise<void> => {
  return safeInvoke("queue_insert_next", { path });
};

export const removeFromQueue = (index: number): Promise<string | null> => {
  return safeInvoke<string | null>("remove_from_queue", { index });
};

export const moveQueueTrack = (from: number, to: number): Promise<void> => {
  return safeInvoke("move_queue_track", { from, to });
};

export const clearQueue = (): Promise<void> => {
  return safeInvoke("clear_queue");
};

export const getQueueTracks = (): Promise<QueueTrackState> => {
  return safeInvoke<QueueTrackState>("get_queue_tracks");
};

export const playTrackFromQueue = (index: number): Promise<void> => {
  return safeInvoke("play_track_from_queue", { index });
};

// ── Playlist export / import ─────────────────────────────────────────────────

export const exportPlaylist = (
  playlistId: string,
  path: string,
  exportFormat: string
): Promise<void> => {
  return safeInvoke("export_playlist", { playlistId, path, exportFormat });
};

export const importPlaylist = (path: string, name?: string): Promise<ImportResult> => {
  return safeInvoke<ImportResult>("import_playlist", { path, name });
};

// ── Dialog helpers for export / import ───────────────────────────────────────

export const savePlaylistDialog = async (defaultName?: string): Promise<string | null> => {
  await tauriInitialized;
  if (!openFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({
    title: "Export Playlist",
    defaultPath: defaultName,
    filters: [
      { name: "M3U Playlist", extensions: ["m3u"] },
      { name: "Wave Playlist (JSON)", extensions: ["json"] },
    ],
  });
};

export const openPlaylistDialog = async (): Promise<string | null> => {
  await tauriInitialized;
  if (!openFn) {
    throw new Error(TAURI_UNAVAILABLE);
  }
  const selected = await openFn({
    multiple: false,
    filters: [{ name: "Playlists", extensions: ["m3u", "m3u8", "json"] }],
    title: "Import Playlist",
  });
  if (selected === null) return null;
  if (typeof selected === "string") return selected;
  return null;
};

// ── Queue / Playback Mode commands ────────────────────────────────────────────

export const getQueue = (): Promise<QueueState> => {
  return safeInvoke<QueueState>("get_queue");
};

export const playNext = (): Promise<string | null> => {
  return safeInvoke<string | null>("play_next");
};

export const playPrevious = (): Promise<string | null> => {
  return safeInvoke<string | null>("play_previous");
};

export const setShuffle = (enabled: boolean): Promise<void> => {
  return safeInvoke("set_shuffle", { enabled });
};

export const setRepeat = (mode: "off" | "one" | "all"): Promise<void> => {
  return safeInvoke("set_repeat", { mode });
};

export const getPlaybackMode = (): Promise<PlaybackMode> => {
  return safeInvoke<PlaybackMode>("get_playback_mode");
};

// ── Equalizer ─────────────────────────────────────────────────────────────────

export interface EqSettings {
  bands: number[];
  enabled: boolean;
}

export const EQ_BAND_LABELS = ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"] as const;

export const EQ_PRESETS: { id: string; label: string; bands: number[] }[] = [
  { id: "flat", label: "Flat", bands: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
  { id: "bass-boost", label: "Bass boost", bands: [4, 4, 2, 0, 0, 0, 0, 0, 0, 0] },
  { id: "bass-cut", label: "Bass cut", bands: [-4, -4, -2, 0, 0, 0, 0, 0, 0, 0] },
  { id: "rock", label: "Rock", bands: [3, 2, 0, -1, -1, 0, 1, 2, 3, 2] },
  { id: "pop", label: "Pop", bands: [1, 1, 2, 3, 3, 2, 1, 1, 1, 1] },
  { id: "jazz", label: "Jazz", bands: [2, 2, 1, 1, 0, 0, 0, 1, 1, 1] },
  { id: "classical", label: "Classical", bands: [0, 0, 0, 0, 0, 0, 0, 1, 2, 2] },
  { id: "vocal", label: "Vocal", bands: [-2, -2, -1, 1, 3, 4, 3, 1, -1, -2] },
  { id: "loudness", label: "Loudness", bands: [5, 4, 2, 0, -1, 0, 1, 2, 3, 4] },
  { id: "headphones", label: "Headphones", bands: [0, 0, 0, 1, 1, 0, -1, -1, 0, 0] },
];

export const getEqSettings = (): Promise<EqSettings> => {
  return safeInvoke<EqSettings>("get_eq_settings");
};

export const setEqBands = (bands: number[]): Promise<void> => {
  return safeInvoke("set_eq_bands", { bands });
};

export const setEqEnabled = (enabled: boolean): Promise<void> => {
  return safeInvoke("set_eq_enabled", { enabled });
};

export const getCrossfadeDuration = (): Promise<number> => {
  return safeInvoke<number>("get_crossfade_duration");
};

export const setCrossfadeDuration = (duration: number): Promise<void> => {
  return safeInvoke("set_crossfade_duration", { duration });
};

export const getGaplessEnabled = (): Promise<boolean> => {
  return safeInvoke<boolean>("get_gapless_enabled");
};

export const setGaplessEnabled = (enabled: boolean): Promise<void> => {
  return safeInvoke("set_gapless_enabled", { enabled });
};

// ── Audio Output Devices ──────────────────────────────────────────────────────

export const listOutputDevices = (): Promise<string[]> => {
  return safeInvoke<string[]>("list_output_devices");
};

export const setOutputDevice = (deviceName: string): Promise<void> => {
  return safeInvoke("set_output_device", { deviceName });
};

// ── OS Media Controls ─────────────────────────────────────────────────────────

export interface MediaMetadata {
  title?: string | null;
  artist?: string | null;
  album?: string | null;
  duration_seconds?: number | null;
  cover_url?: string | null;
}

/**
 * Push track metadata to the OS media interface (macOS Control Center,
 * Windows SMTC, Linux MPRIS, Android MediaSession notification).
 * Call this whenever the playing track changes.
 */
export const updateMediaMetadata = (metadata: MediaMetadata): Promise<void> => {
  return safeInvoke("update_media_metadata", { metadata });
};

/**
 * Push a playback-position tick to the OS media interface so the system
 * overlay / Control Center / MPRIS shows an accurate, moving progress bar.
 * Call this periodically (e.g. every 500 ms) while the track is playing.
 */
export const updateMediaPosition = (position_seconds: number, is_playing: boolean): Promise<void> => {
  return safeInvoke("update_media_position", { position_seconds, is_playing });
};

/** Clear the OS media session when no track is loaded. */
export const clearMediaSession = (): Promise<void> => {
  return safeInvoke("clear_media_session");
};

/**
 * Listen for OS media control events (play, pause, next, previous, seek).
 * Returns an unlisten function — call it when your component unmounts.
 *
 * Usage:
 *   const unlisten = await listenToMediaControls({ onPlay, onPause, onNext, ... });
 *   // later:
 *   unlisten();
 */
export interface MediaControlHandlers {
  onPlay?: () => void;
  onPause?: () => void;
  onToggle?: () => void;
  onNext?: () => void;
  onPrevious?: () => void;
  onStop?: () => void;
  onSeekRelative?: (direction: "forward" | "backward") => void;
  onSeekBy?: (seconds: number) => void;
  onSetPosition?: (seconds: number) => void;
  /** Android notification "shuffle" button tapped. */
  onShuffle?: () => void;
  /** Android notification "repeat" button tapped. */
  onRepeat?: () => void;
}

export const listenToSyncProgress = async (
  onProgress: (payload: {
    playlist_id?: string;
    phase?: string;
    scanned?: number;
    to_add?: number;
    to_remove?: number;
    processed?: number;
    extracted?: number;
    added?: number;
    removed?: number;
  }) => void,
): Promise<() => void> => {
  await tauriInitialized;
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen("sync-progress", (e) => {
    onProgress((e.payload ?? {}) as {
      playlist_id?: string;
      phase?: string;
      scanned?: number;
      to_add?: number;
      to_remove?: number;
      processed?: number;
      extracted?: number;
      added?: number;
      removed?: number;
    });
  });
  return () => {
    unlisten();
  };
};

export const listenToMediaControls = async (
  handlers: MediaControlHandlers
): Promise<() => void> => {
  await tauriInitialized;
  const { listen } = await import("@tauri-apps/api/event");

  const unlisteners = await Promise.all([
    handlers.onPlay     && listen("media-control-play",     () => handlers.onPlay!()),
    handlers.onPause    && listen("media-control-pause",    () => handlers.onPause!()),
    handlers.onToggle   && listen("media-control-toggle",   () => handlers.onToggle!()),
    handlers.onNext     && listen("media-control-next",     () => handlers.onNext!()),
    handlers.onPrevious && listen("media-control-previous", () => handlers.onPrevious!()),
    handlers.onStop     && listen("media-control-stop",     () => handlers.onStop!()),
    handlers.onSeekRelative && listen<string>(
      "media-control-seek-relative",
      (e) => handlers.onSeekRelative!(e.payload as "forward" | "backward")
    ),
    handlers.onSeekBy && listen<number>(
      "media-control-seek-by",
      (e) => handlers.onSeekBy!(e.payload)
    ),
    handlers.onSetPosition && listen<number>(
      "media-control-set-position",
      (e) => handlers.onSetPosition!(e.payload)
    ),
  ]);

  // Android MediaSession / Bluetooth / notification buttons arrive via the
  // media-session plugin rather than the desktop `media-control-*` events.
  let unregisterPlugin: (() => void) | null = null;
  try {
    const { isAndroid } = await import("./platform");
    if (await isAndroid()) {
      const { onMediaSessionAction } = await import("./mediaSessionPlugin");
      const listener = await onMediaSessionAction((event) => {
        switch (event.action) {
          case "play":
            handlers.onPlay?.();
            break;
          case "pause":
            handlers.onPause?.();
            break;
          case "stop":
            handlers.onStop?.();
            break;
          case "next":
            handlers.onNext?.();
            break;
          case "previous":
            handlers.onPrevious?.();
            break;
          case "seek":
            if (typeof event.seekPosition === "number") {
              handlers.onSetPosition?.(event.seekPosition);
            }
            break;
          case "shuffle":
            handlers.onShuffle?.();
            break;
          case "repeat":
            handlers.onRepeat?.();
            break;
        }
      });
      if (listener) {
        unregisterPlugin = () => {
          void listener.unregister();
        };
      }
    }
  } catch (error) {
    console.warn("Failed to attach Android media-session listener:", error);
  }

  return () => {
    unlisteners.forEach((u) => u && u());
    unregisterPlugin?.();
  };
};

/** What the window close button does. */
export type CloseAction = "quit" | "hide_window";

export const getCloseAction = (): Promise<CloseAction> =>
  safeInvoke<CloseAction>("get_close_action");

export const setCloseAction = (action: CloseAction): Promise<CloseAction> =>
  safeInvoke<CloseAction>("set_close_action", { action });

export const toggleCloseAction = (): Promise<CloseAction> =>
  safeInvoke<CloseAction>("toggle_close_action");

// ── Media folders ─────────────────────────────────────────────────────────────

export const listMediaFolders = (): Promise<string[]> =>
  safeInvoke<string[]>("list_media_folders");

export const saveMediaFolder = (path: string): Promise<void> =>
  safeInvoke("save_media_folder", { path });

export const removeMediaFolder = (path: string): Promise<void> =>
  safeInvoke("remove_media_folder", { path });

export const scanMediaFolder = (folder: string): Promise<string[]> =>
  safeInvoke<string[]>("scan_media_folder", { folder });

export interface ScanImportResult {
  imported: number;
  errors: string[];
}

export const importScannedAudio = (
  paths: string[],
  playlistId: string,
): Promise<ScanImportResult> =>
  safeInvoke<ScanImportResult>("import_scanned_audio", { paths, playlistId });

export interface SyncPlaylistResult {
  added: number;
  removed: number;
  errors: string[];
}

/** Reconcile a synced playlist with folder contents (add missing, remove gone). */
export const syncPlaylistFolder = (
  playlistId: string,
  scannedPaths?: string[] | null,
): Promise<SyncPlaylistResult> =>
  safeInvoke<SyncPlaylistResult>("sync_playlist_folder", {
    playlistId,
    scannedPaths: scannedPaths ?? null,
  });

export const isFolderSetupDismissed = (): Promise<boolean> =>
  safeInvoke<boolean>("is_folder_setup_dismissed");

export const dismissFolderSetup = (): Promise<void> =>
  safeInvoke("dismiss_folder_setup");

/** Recursively scan a directory URI for audio files.
 *  - Android SAF `content://…/tree/…` → native DocumentsContract walk
 *    (`tauri-plugin-fs` readDir cannot list content:// trees).
 *  - Filesystem paths → `@tauri-apps/plugin-fs` readDir. */
export const scanDirectoryRecursive = async (dirUri: string): Promise<string[]> => {
  const trimmed = dirUri.trim();
  if (trimmed.startsWith("content://")) {
    await tauriInitialized;
    if (!invokeFn) {
      throw new Error(TAURI_UNAVAILABLE);
    }
    try {
      return await invokeFn<string[]>("scan_saf_folder", { uri: trimmed });
    } catch (err) {
      throw new Error(
        invokeErrorMessage(err, "Failed to scan the selected folder"),
      );
    }
  }

  const { readDir } = await import("@tauri-apps/plugin-fs");
  const results: string[] = [];
  let rootError: unknown = null;
  let readableDirs = 0;

  const AUDIO_EXTENSIONS = new Set([
    "mp3", "flac", "ogg", "opus", "wav", "m4a", "m4b", "aac",
    "aiff", "alac", "caf", "mka", "wma", "weba",
  ]);

  const isAudioFile = (name: string): boolean => {
    const dot = name.lastIndexOf(".");
    if (dot < 0) return false;
    const ext = name.slice(dot + 1).toLowerCase();
    return AUDIO_EXTENSIONS.has(ext);
  };

  /** Build a child tree URI for SAF content:// URIs.
   *  Parent: content://.../tree/primary%3AMusic
   *  Child:  content://.../tree/primary%3AMusic%2Fdirname           */
  const childTreeUri = (parentUri: string, childName: string): string =>
    parentUri + "%2F" + encodeURIComponent(childName);

  /** Build a child document URI (for files) from a SAF tree URI.
   *  Parent tree: content://.../tree/primary%3AMusic
   *  Child doc:   content://.../document/primary%3AMusic%2Ffile.mp3  */
  const childDocUri = (parentUri: string, childName: string): string =>
    parentUri.replace("/tree/", "/document/") + "%2F" + encodeURIComponent(childName);

  const walk = async (uri: string, isRoot: boolean) => {
    try {
      const entries = await readDir(uri);
      readableDirs += 1;
      for (const entry of entries) {
        if (!entry.name) continue;
        if (entry.isDirectory) {
          await walk(childTreeUri(uri, entry.name), false);
        } else if (isAudioFile(entry.name)) {
          results.push(childDocUri(uri, entry.name));
        }
      }
    } catch (err) {
      if (isRoot) {
        rootError = err;
      }
      // Skip nested directories we can't read (permission denied, etc.)
    }
  };

  await walk(trimmed, true);

  if (rootError != null && readableDirs === 0) {
    throw new Error(
      invokeErrorMessage(
        rootError,
        "Failed to read the selected folder (permission or unsupported URI)",
      ),
    );
  }

  return results;
};
