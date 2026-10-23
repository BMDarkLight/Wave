/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// Remote sources (Plex today): setup, the source list, refresh, the shared
// cache, and whether a remote track can play right now.

import { createContext } from "react";
import { listenTo, safeInvoke, type Track } from "./player";

/** Whether Wave may use the network for remote music right now. */
export type GateState = "allowed" | "disabled" | "cellular";

export type SourceState =
  "ok" | "syncing" | "unreachable" | "signed_out" | "blocked";

export interface RemoteStatus {
  gate: GateState;
  sources: { id: string; state: SourceState }[];
}

export interface RemoteSource {
  id: string;
  kind: string;
  display_name: string;
  state: SourceState;
  track_count: number;
  last_sync_at: number | null;
  last_error: string | null;
  /** How to sign in again: "plex_tv", "plex_token", "password" or "none". */
  reauth: string;
}

export interface PlexDirectServer {
  server_id: string;
  name: string;
  libraries: PlexLibrary[];
}

export interface PlexSignIn {
  session_id: string;
  auth_url: string;
}

export interface PlexServer {
  id: string;
  name: string;
  owned: boolean;
}

export interface PlexLibrary {
  key: string;
  title: string;
}

export interface RefreshTicket {
  started: boolean;
  retry_after_seconds: number;
}

export interface RemoteSyncProgress {
  source_id: string;
  synced: number;
  total: number | null;
}

export const plexBeginSignIn = (): Promise<PlexSignIn> =>
  safeInvoke<PlexSignIn>("plex_begin_sign_in");

export const plexPollSignIn = (
  sessionId: string,
): Promise<"pending" | "done" | "expired"> =>
  safeInvoke("plex_poll_sign_in", { sessionId });

export const plexListServers = (sessionId: string): Promise<PlexServer[]> =>
  safeInvoke<PlexServer[]>("plex_list_servers", { sessionId });

export const plexListLibraries = (
  sessionId: string,
  serverId: string,
): Promise<PlexLibrary[]> =>
  safeInvoke<PlexLibrary[]>("plex_list_libraries", { sessionId, serverId });

export const plexAddSource = (
  sessionId: string,
  serverId: string,
  libraryKeys: string[],
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("plex_add_source", {
    sessionId,
    serverId,
    libraryKeys,
  });

export const plexReauthorizeSource = (
  sourceId: string,
  sessionId: string,
): Promise<void> =>
  safeInvoke<void>("plex_reauthorize_source", { sourceId, sessionId });

/** Finds a Plex server at an address, without plex.tv. */
export const plexProbeDirect = (
  url: string,
  token: string,
): Promise<PlexDirectServer> =>
  safeInvoke<PlexDirectServer>("plex_probe_direct", {
    url,
    token: token.trim() || null,
  });

export const plexAddDirectSource = (
  url: string,
  token: string,
  libraryKeys: string[],
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("plex_add_direct_source", {
    url,
    token: token.trim() || null,
    libraryKeys,
  });

export const plexSetDirectToken = (
  sourceId: string,
  token: string,
): Promise<void> =>
  safeInvoke<void>("plex_set_direct_token", { sourceId, token });

/** Adds a Subsonic-compatible server (Navidrome, Airsonic, Gonic). */
export const subsonicAddSource = (
  url: string,
  username: string,
  password: string,
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("subsonic_add_source", {
    url,
    username: username.trim(),
    password,
  });

/**
 * Adds a WebDAV folder (Nextcloud, ownCloud, a NAS share). The username is
 * optional; leave it blank for a folder that needs no sign-in.
 */
export const webdavAddSource = (
  url: string,
  username: string,
  password: string,
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("webdav_add_source", {
    url,
    username: username.trim(),
    password,
  });

/** A DLNA or UPnP media server found on the local network. */
export interface DlnaServer {
  location: string;
  name: string;
}

export const dlnaDiscover = (): Promise<DlnaServer[]> =>
  safeInvoke<DlnaServer[]>("dlna_discover");

/** Adds a media server by its description address or a bare host and port. */
export const dlnaAddSource = (address: string): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("dlna_add_source", { address });

/** Adds an Audius account's uploads, favorites and reposts by its handle. */
export const audiusAddSource = (handle: string): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("audius_add_source", { handle: handle.trim() });

export const kodiAddSource = (
  url: string,
  username: string,
  password: string,
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("kodi_add_source", {
    url,
    username: username.trim(),
    password,
  });

/** Signs a Subsonic, Jellyfin, Emby, WebDAV or Kodi source in again. */
export const setSourcePassword = (
  sourceId: string,
  password: string,
): Promise<void> =>
  safeInvoke<void>("set_source_password", { sourceId, password });

/** A Jellyfin or Emby server signed in to, waiting for libraries to be picked. */
export interface JellyfinServer {
  session_id: string;
  name: string;
  libraries: PlexLibrary[];
}

export const jellyfinSignIn = (
  url: string,
  username: string,
  password: string,
): Promise<JellyfinServer> =>
  safeInvoke<JellyfinServer>("jellyfin_sign_in", {
    url,
    username: username.trim(),
    password,
  });

export const jellyfinAddSource = (
  sessionId: string,
  libraryIds: string[],
): Promise<RemoteSource> =>
  safeInvoke<RemoteSource>("jellyfin_add_source", { sessionId, libraryIds });

/** Whether the library is shared on the network, as what and where. */
export interface ShareStatus {
  running: boolean;
  name: string;
  addresses: string[];
  track_count: number;
  error: string | null;
}

export const getShareStatus = (): Promise<ShareStatus> =>
  safeInvoke<ShareStatus>("get_share_status");

export const listRemoteSources = (): Promise<RemoteSource[]> =>
  safeInvoke<RemoteSource[]>("list_remote_sources");

export const removeRemoteSource = (sourceId: string): Promise<void> =>
  safeInvoke<void>("remove_remote_source", { sourceId });

export const getRemoteStatus = (): Promise<RemoteStatus> =>
  safeInvoke<RemoteStatus>("get_remote_status");

export const requestLibraryRefresh = (): Promise<RefreshTicket> =>
  safeInvoke<RefreshTicket>("request_library_refresh");

/** Deletes every cached track not in use. Resolves to the bytes freed. */
export const clearSourceCache = (): Promise<number> =>
  safeInvoke<number>("clear_source_cache");

export const getSourceCacheUsage = (): Promise<number> =>
  safeInvoke<number>("get_source_cache_usage");

/** Keep a remote track for good: it becomes a local file. */
export const downloadRemoteTrack = (path: string): Promise<Track> =>
  safeInvoke<Track>("download_remote_track", { path });

export const listenToRemoteStatus = (onChange: () => void) =>
  listenTo<unknown>("remote-status-changed", () => onChange());

export const listenToRemoteSync = (
  onProgress: (progress: RemoteSyncProgress) => void,
) => listenTo<RemoteSyncProgress>("remote-sync-progress", onProgress);

export const listenToLibraryRefreshFinished = (onFinished: () => void) =>
  listenTo<unknown>("library-refresh-finished", () => onFinished());

/** Library content changed in the background (a sync, a download). */
export const listenToLibraryChanged = (onChange: () => void) =>
  listenTo<unknown>("library-changed", () => onChange());

export const listenToRemoteStreamFailed = (
  onFailed: (failure: { path: string; message: string }) => void,
) =>
  listenTo<{ path: string; message: string }>("remote-stream-failed", onFailed);

/** A remote track finished downloading into the cache. */
export const listenToRemoteTrackCached = (onCached: (path: string) => void) =>
  listenTo<{ path: string }>("remote-track-cached", (e) => onCached(e.path));

export const RemoteStatusContext = createContext<RemoteStatus | null>(null);

export function isRemote(track: Pick<Track, "source_state">): boolean {
  return track.source_state === "remote";
}

/**
 * Why a track can't start right now, or null when it can. Local tracks,
 * downloads and fully cached remote tracks always can.
 */
export function unavailableReason(
  track: Track,
  status: RemoteStatus | null,
): string | null {
  if (!isRemote(track) || track.cache_state === "complete") return null;
  if (track.remote_missing) return "Removed from the server";
  if (status?.gate === "disabled") return "Outside Sourcing is off";
  if (status?.gate === "cellular") return "Your servers are set to Wi-Fi only";
  const state = status?.sources.find(
    (s) => s.id === track.remote_source_id,
  )?.state;
  if (state === "signed_out") return "Sign in again in Settings";
  if (state === "unreachable") return "Can't reach the server";
  return null;
}
