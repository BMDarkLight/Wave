/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState, type ReactNode } from "react";
import {
  BiCheck,
  BiChevronLeft,
  BiDotsHorizontalRounded,
  BiGlobe,
} from "react-icons/bi";
import {
  getOnlineAlbum,
  getOnlineArtist,
  type OnlineAlbumPage,
  type OnlineArtistPage,
  type SourceTrack,
  type StreamStage,
} from "../utils/player";
import { StreamProgress } from "./SourceResults";
import { formatTime } from "../utils/format";
import { sourceTrackKey } from "../hooks/useSourceSearch";
import type { ContextMenuAnchor } from "./ContextMenu";

const providerName = (provider: string) =>
  provider === "deezer" ? "Deezer" : provider;

/** Marks a page as fetched from a catalog rather than from the library. */
function OnlineBadge({ provider }: { provider: string }) {
  return (
    <span
      className="online-page-badge"
      title={`Fetched from ${providerName(provider)}, not in your library`}
    >
      <BiGlobe /> Online · {providerName(provider)}
    </span>
  );
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      className="page-back-btn"
      onClick={onBack}
      type="button"
      title="Back"
      aria-label="Back"
    >
      <BiChevronLeft />
    </button>
  );
}

/** Loads a page whenever its id changes, dropping answers that come late. */
function useOnlinePage<T>(load: () => Promise<T>, key: string) {
  const [page, setPage] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    setPage(null);
    setError(null);
    load()
      .then((found) => current && setPage(found))
      .catch(
        (e) => current && setError(e instanceof Error ? e.message : String(e)),
      );
    return () => {
      current = false;
    };
    // The key stands for everything `load` reads.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return { page, error };
}

function Waiting({
  onBack,
  error,
  what,
}: {
  onBack: () => void;
  error: string | null;
  what: string;
}) {
  return (
    <div className="main-content page-with-float-back">
      <BackButton onBack={onBack} />
      <div className="empty-state">
        {error ? (
          <>
            <h2>Couldn&apos;t load this {what}</h2>
            <p>{error}</p>
          </>
        ) : (
          <>
            <div className="empty-icon">
              <span className="import-spinner" />
            </div>
            <h2>Loading {what}…</h2>
          </>
        )}
      </div>
    </div>
  );
}

type TrackActions = {
  /** Keys of tracks being streamed or saved. */
  busy: Record<string, "stream" | "download">;
  /** How far finding each Online play song being streamed has got. */
  stages: Record<string, StreamStage>;
  onPlay: (track: SourceTrack) => void;
  onOpenTrackMenu: (track: SourceTrack, anchor: ContextMenuAnchor) => void;
};

function OnlineTrackList({
  tracks,
  busy,
  stages,
  onPlay,
  onOpenTrackMenu,
  meta,
}: TrackActions & {
  tracks: SourceTrack[];
  meta: (track: SourceTrack) => ReactNode;
}) {
  return (
    <div
      className="track-list track-list-compact"
      style={
        {
          "--track-grid": "36px minmax(0, 1fr) 78px 40px",
        } as React.CSSProperties
      }
    >
      <div className="track-list-header">
        <div className="track-col-index">#</div>
        <div className="track-title-cell">Title</div>
        <div className="track-duration track-duration-header">Duration</div>
      </div>
      {tracks.map((track, i) => {
        const state = busy[sourceTrackKey(track)];
        const openMenu = (anchor: ContextMenuAnchor) =>
          onOpenTrackMenu(track, anchor);
        return (
          <div
            key={sourceTrackKey(track)}
            className={`track-item${state ? " busy" : ""}`}
            onClick={() => !state && onPlay(track)}
            onContextMenu={(event) => {
              event.preventDefault();
              openMenu({
                top: event.clientY,
                left: event.clientX,
                flipAbove: event.clientY,
              });
            }}
          >
            <div className="track-col-index">
              {state ? <span className="import-spinner" /> : i + 1}
            </div>
            <div className="track-title-cell">
              {track.artwork_url ? (
                <img
                  className="track-thumb"
                  src={track.artwork_url}
                  alt=""
                  loading="lazy"
                />
              ) : (
                <div className="track-thumb">
                  {track.title.slice(0, 1).toUpperCase()}
                </div>
              )}
              <div>
                <div className="track-name">{track.title}</div>
                <div className="track-meta">
                  {meta(track)}
                  {track.already_in_library && (
                    <span className="online-owned" title="In your library">
                      {" "}
                      <BiCheck />
                    </span>
                  )}
                  {!track.is_full_length && (
                    <span className="online-preview"> · 30s preview</span>
                  )}
                </div>
              </div>
            </div>
            <div className="track-duration">
              {state === "stream" ? (
                <StreamProgress stage={stages[sourceTrackKey(track)]} />
              ) : (
                formatTime(track.duration_seconds)
              )}
            </div>
            <div className="track-actions-cell">
              <div className="track-actions-hover">
                <button
                  className="track-action-btn"
                  type="button"
                  title="More"
                  onClick={(event) => {
                    event.stopPropagation();
                    const rect = event.currentTarget.getBoundingClientRect();
                    openMenu({
                      top: rect.bottom + 4,
                      flipAbove: rect.top - 4,
                      right: window.innerWidth - rect.right,
                    });
                  }}
                >
                  <BiDotsHorizontalRounded />
                </button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}

const RECORD_TYPES: Record<string, string> = {
  album: "Album",
  single: "Single",
  ep: "EP",
  compile: "Compilation",
};

export function OnlineArtistView({
  provider,
  id,
  name,
  onBack,
  onOpenAlbum,
  ...actions
}: TrackActions & {
  provider: string;
  id: string;
  name: string;
  onBack: () => void;
  onOpenAlbum: (provider: string, id: string, title: string) => void;
}) {
  const { page, error } = useOnlinePage<OnlineArtistPage>(
    () => getOnlineArtist(provider, id),
    `${provider}:${id}`,
  );
  if (!page) return <Waiting onBack={onBack} error={error} what="artist" />;

  return (
    <div className="main-content page-with-float-back playlist-page">
      <BackButton onBack={onBack} />
      <div className="artist-hero online-artist-hero">
        {page.picture_url && (
          <img
            className="online-artist-picture"
            src={page.picture_url}
            alt=""
          />
        )}
        <div>
          <OnlineBadge provider={provider} />
          <h1 className="artist-hero-title">{page.name || name}</h1>
          <div className="artist-hero-meta">
            {page.albums.length > 0 && (
              <span>
                {page.albums.length} release
                {page.albums.length !== 1 ? "s" : ""}
              </span>
            )}
            {page.fans != null && (
              <span> · {page.fans.toLocaleString()} fans</span>
            )}
          </div>
        </div>
      </div>

      {page.top_tracks.length > 0 && (
        <section className="artist-section">
          <h2 className="artist-section-title">Top songs</h2>
          <OnlineTrackList
            tracks={page.top_tracks}
            meta={(track) => track.album}
            {...actions}
          />
        </section>
      )}

      {page.albums.length > 0 && (
        <section className="artist-section">
          <h2 className="artist-section-title">Releases</h2>
          <div className="artist-album-grid">
            {page.albums.map((album) => (
              <button
                key={album.id}
                className="artist-album-card"
                onClick={() => onOpenAlbum(provider, album.id, album.title)}
                type="button"
              >
                {album.cover_url ? (
                  <img
                    className="artist-album-card-cover"
                    src={album.cover_url}
                    alt=""
                    loading="lazy"
                  />
                ) : (
                  <div className="artist-album-card-cover" />
                )}
                <div className="artist-album-card-name">{album.title}</div>
                <div className="artist-album-card-meta">
                  {[
                    album.year,
                    album.record_type && RECORD_TYPES[album.record_type],
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </div>
              </button>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

export function OnlineAlbumView({
  provider,
  id,
  name,
  onBack,
  onOpenArtist,
  ...actions
}: TrackActions & {
  provider: string;
  id: string;
  name: string;
  onBack: () => void;
  onOpenArtist: (provider: string, id: string, name: string) => void;
}) {
  const { page, error } = useOnlinePage<OnlineAlbumPage>(
    () => getOnlineAlbum(provider, id),
    `${provider}:${id}`,
  );
  if (!page) return <Waiting onBack={onBack} error={error} what="album" />;

  const totalMin = Math.round(
    page.tracks.reduce((sum, t) => sum + (t.duration_seconds ?? 0), 0) / 60,
  );
  const artistRef = page.artist_ref;
  return (
    <div className="main-content page-with-float-back playlist-page">
      <BackButton onBack={onBack} />
      <div className="album-hero">
        {page.cover_url ? (
          <img className="album-hero-cover" src={page.cover_url} alt="" />
        ) : (
          <div className="album-hero-cover" />
        )}
        <div className="album-hero-info">
          <OnlineBadge provider={provider} />
          <h1 className="album-hero-title">{page.title || name}</h1>
          <div className="album-hero-meta">
            {artistRef ? (
              <button
                className="album-hero-artist-btn"
                onClick={() => onOpenArtist(provider, artistRef, page.artist)}
                type="button"
              >
                {page.artist}
              </button>
            ) : (
              <span>{page.artist}</span>
            )}
            {page.year && <span>· {page.year}</span>}
            <span>
              · {page.tracks.length} song{page.tracks.length !== 1 ? "s" : ""},
              about {totalMin} min
            </span>
          </div>
        </div>
      </div>
      <section className="playlist-container">
        <OnlineTrackList
          tracks={page.tracks}
          meta={(track) => track.artist}
          {...actions}
        />
      </section>
    </div>
  );
}
