/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// The Library tab on the narrow layout: every collection in one list, so
// all songs, favorites, listening history and playlists live in one place
// instead of being split between the tabs and a drawer.

import type { ReactNode } from "react";
import PullToRefresh from "./PullToRefresh";
import {
  BiAlbum,
  BiBarChartAlt2,
  BiChevronRight,
  BiCog,
  BiHistory,
  BiImport,
  BiDevices,
  BiListUl,
  BiMusic,
  BiPlus,
  BiSolidHeart,
  BiSync,
  BiUser,
} from "react-icons/bi";
import type { PlaylistInfo } from "../utils/player";

interface MobileLibraryPageProps {
  libraryPlaylist: PlaylistInfo | null;
  favoritesPlaylist: PlaylistInfo | null;
  userPlaylists: PlaylistInfo[];
  onSelectPlaylist: (id: string) => void;
  onGoRecentlyPlayed: () => void;
  onGoMostPlayed: () => void;
  /** The Devices page; its row is hidden while Outside Sourcing is off. */
  onGoDevices: (() => void) | null;
  /** Paired Waves in reach right now. */
  devicesOnline: number;
  /** Waves waiting for an answer to share libraries. */
  devicesAsking: number;
  onGoAlbums: () => void;
  onGoArtists: () => void;
  onOpenSettings: () => void;
  onImportPlaylist: () => void;
  onCreatePlaylist: () => void;
  onRefresh: () => void;
  refreshing: boolean;
  refreshCooldownSeconds: number;
  /** Whether each paired device's playlist can play right now, by id. */
  deviceOnline: Record<string, boolean>;
}

function Row({
  icon,
  tone,
  label,
  count,
  synced = false,
  device,
  note,
  alert = false,
  onClick,
}: {
  icon: ReactNode;
  tone?: "favorites";
  label: string;
  count?: number;
  /** Kept in step with a music folder. */
  synced?: boolean;
  /** A paired device's playlist, and whether that device is in reach. */
  device?: { online: boolean };
  /** A short status after the label, highlighted with `alert`. */
  note?: string;
  alert?: boolean;
  onClick: () => void;
}) {
  return (
    <button className="mlib-row" onClick={onClick} type="button">
      <span
        className={`mlib-row-icon${tone ? ` mlib-row-icon-${tone}` : ""}`}
        aria-hidden
      >
        {icon}
      </span>
      <span className="mlib-row-label-wrap">
        <span className="mlib-row-label">{label}</span>
        {synced && (
          <BiSync
            className="mlib-row-sync"
            title="Synced with a folder"
            aria-label="Synced with a folder"
          />
        )}
        {note && (
          <span className={`mlib-row-device${alert ? " mlib-row-alert" : ""}`}>
            {!alert && (
              <span className="device-status-dot online" aria-hidden />
            )}
            {note}
          </span>
        )}
        {device && (
          <span className="mlib-row-device">
            <span
              className={`device-status-dot${device.online ? " online" : ""}`}
              aria-hidden
            />
            {device.online ? "Available" : "Offline"}
          </span>
        )}
      </span>
      {count !== undefined && (
        <span className="mlib-row-count">{count.toLocaleString()}</span>
      )}
      <BiChevronRight className="mlib-row-chevron" aria-hidden />
    </button>
  );
}

export default function MobileLibraryPage({
  libraryPlaylist,
  favoritesPlaylist,
  userPlaylists,
  onSelectPlaylist,
  onGoRecentlyPlayed,
  onGoMostPlayed,
  onGoDevices,
  devicesOnline,
  devicesAsking,
  onGoAlbums,
  onGoArtists,
  onOpenSettings,
  onImportPlaylist,
  onCreatePlaylist,
  onRefresh,
  refreshing,
  refreshCooldownSeconds,
  deviceOnline,
}: MobileLibraryPageProps) {
  return (
    <main className="main-content mobile-library-page">
      <PullToRefresh
        onRefresh={onRefresh}
        refreshing={refreshing}
        cooldownSeconds={refreshCooldownSeconds}
      >
        <div className="mlib-header">
          <h1>Your Library</h1>
          <div className="mlib-header-actions">
            <button
              className="mlib-icon-btn"
              onClick={onImportPlaylist}
              type="button"
              title="Import playlist"
              aria-label="Import playlist"
            >
              <BiImport />
            </button>
            <button
              className="mlib-icon-btn"
              onClick={onCreatePlaylist}
              type="button"
              title="Create playlist"
              aria-label="Create playlist"
            >
              <BiPlus />
            </button>
            <button
              className="mlib-icon-btn"
              onClick={onOpenSettings}
              type="button"
              title="Settings"
              aria-label="Settings"
            >
              <BiCog />
            </button>
          </div>
        </div>

        <div className="mlib-list">
          {libraryPlaylist && (
            <Row
              icon={<BiMusic />}
              label="All songs"
              count={libraryPlaylist.track_count}
              onClick={() => onSelectPlaylist(libraryPlaylist.id)}
            />
          )}
          {favoritesPlaylist && (
            <Row
              icon={<BiSolidHeart />}
              tone="favorites"
              label="Favorites"
              count={favoritesPlaylist.track_count}
              onClick={() => onSelectPlaylist(favoritesPlaylist.id)}
            />
          )}
          <Row icon={<BiAlbum />} label="Albums" onClick={onGoAlbums} />
          <Row icon={<BiUser />} label="Artists" onClick={onGoArtists} />
          <Row
            icon={<BiHistory />}
            label="Recently Played"
            onClick={onGoRecentlyPlayed}
          />
          <Row
            icon={<BiBarChartAlt2 />}
            label="Most Played"
            onClick={onGoMostPlayed}
          />
          {onGoDevices && (
            <Row
              icon={<BiDevices />}
              label="Devices"
              note={
                devicesAsking > 0
                  ? `${devicesAsking} waiting for you`
                  : devicesOnline > 0
                    ? `${devicesOnline} in reach`
                    : undefined
              }
              alert={devicesAsking > 0}
              onClick={onGoDevices}
            />
          )}
        </div>

        <h2 className="mlib-section-title">Playlists</h2>
        <div className="mlib-list">
          {userPlaylists.length === 0 ? (
            <div className="mlib-empty">
              <p>No playlists yet</p>
              <button
                className="btn-ghost btn-sm"
                onClick={onCreatePlaylist}
                type="button"
              >
                Create one
              </button>
            </div>
          ) : (
            userPlaylists.map((playlist) => (
              <Row
                key={playlist.id}
                icon={playlist.peer_source_id ? <BiDevices /> : <BiListUl />}
                label={playlist.name}
                count={playlist.track_count}
                synced={!!playlist.sync_folder}
                device={
                  playlist.peer_source_id
                    ? { online: !!deviceOnline[playlist.id] }
                    : undefined
                }
                onClick={() => onSelectPlaylist(playlist.id)}
              />
            ))
          )}
        </div>
      </PullToRefresh>
    </main>
  );
}
