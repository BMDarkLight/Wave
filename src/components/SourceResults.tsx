/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import {
  BiCloudDownload,
  BiCheck,
  BiDotsHorizontalRounded,
  BiErrorCircle,
} from "react-icons/bi";
import type { ContextMenuAnchor } from "./ContextMenu";
import type {
  ProviderResults,
  SourceTrack,
  StreamStage,
} from "../utils/player";
import { sourceTrackKey } from "../hooks/useSourceSearch";
import { highlightMatch } from "../utils/search";
import { formatTime } from "../utils/format";

type Props = {
  results: ProviderResults[];
  loading: boolean;
  searched: boolean;
  error: string | null;
  /** Keys of results with a stream or download in flight. */
  busy: Record<string, "stream" | "download">;
  /** How far finding each Online play song being streamed has got. */
  stages: Record<string, StreamStage>;
  query: string;
  /** Play a result — streaming it first if it isn't already owned. */
  onPlay: (track: SourceTrack) => void;
  /** Keep a result in the library. */
  onDownload: (track: SourceTrack) => void;
  /** Open a result's menu, with Go to Artist and Go to Album. */
  onOpenMenu: (track: SourceTrack, anchor: ContextMenuAnchor) => void;
};

const STAGES: Record<StreamStage | "starting", [string, number]> = {
  starting: ["Loading", 0.1],
  searching: ["Searching YouTube", 0.35],
  checking: ["Checking the match", 0.65],
  found: ["Starting", 0.9],
};

/** Where a song being loaded has got: a label over a filling bar. */
export function StreamProgress({ stage }: { stage?: StreamStage }) {
  const [label, fraction] = STAGES[stage ?? "starting"];
  return (
    <span className="stream-progress" role="status" aria-label={label}>
      <span className="stream-progress-label">{label}…</span>
      <span className="stream-progress-bar">
        <span style={{ width: `${fraction * 100}%` }} />
      </span>
    </span>
  );
}

/**
 * Tier 3 results, grouped by provider.
 *
 * A provider that failed or still needs setup is left out: its section only
 * appears when it answered.
 */
export function SourceResults({
  results,
  loading,
  searched,
  error,
  busy,
  stages,
  query,
  onPlay,
  onDownload,
  onOpenMenu,
}: Props) {
  if (loading) {
    return (
      <div className="source-results">
        <div className="source-section-head">
          <span className="import-spinner" />
          <span>Searching sources…</span>
        </div>
      </div>
    );
  }

  if (!searched) return null;

  if (error) {
    return (
      <div className="source-results">
        <div className="source-section-error">
          <BiErrorCircle />
          <span>{error}</span>
        </div>
      </div>
    );
  }

  const answered = results.filter((r) => !r.error);
  const totalHits = answered.reduce((sum, r) => sum + r.tracks.length, 0);
  if (totalHits === 0) {
    return (
      <div className="source-results">
        <div className="source-section-empty">
          No sources had anything matching “{query.trim()}”.
        </div>
      </div>
    );
  }

  return (
    <div className="source-results">
      {answered.map((section) => (
        <section key={section.provider} className="source-section">
          <div className="source-section-head">
            <h3>{section.display_name}</h3>
            <span className="source-section-count">
              {section.tracks.length}
            </span>
          </div>

          {section.tracks.length === 0 ? (
            <div className="source-section-empty">No matches here.</div>
          ) : (
            <div className="source-hit-list">
              {section.tracks.map((track) => {
                const key = sourceTrackKey(track);
                const state = busy[key];
                const owned = !!track.already_in_library;
                return (
                  <div
                    key={key}
                    className={`source-hit ${state ? "busy" : ""}`}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      onOpenMenu(track, {
                        top: event.clientY,
                        left: event.clientX,
                        flipAbove: event.clientY,
                      });
                    }}
                  >
                    <button
                      type="button"
                      className="source-hit-main"
                      disabled={!!state}
                      onClick={() => onPlay(track)}
                      title={
                        owned
                          ? "Already in your library — plays your copy"
                          : track.is_full_length
                            ? "Stream this track"
                            : "Play the 30-second preview"
                      }
                    >
                      {track.artwork_url ? (
                        <img
                          className="track-thumb source-hit-thumb"
                          src={track.artwork_url}
                          alt=""
                          loading="lazy"
                        />
                      ) : (
                        <div className="track-thumb source-hit-thumb source-hit-thumb-empty">
                          {track.title.slice(0, 1).toUpperCase()}
                        </div>
                      )}

                      <div className="source-hit-body">
                        <div className="source-hit-title">
                          {highlightMatch(track.title, query)}
                        </div>
                        <div className="source-hit-meta">
                          {highlightMatch(track.artist, query)}
                          {track.album ? (
                            <>
                              {" · "}
                              {highlightMatch(track.album, query)}
                            </>
                          ) : null}
                        </div>
                        <div className="source-hit-chips">
                          {!track.is_full_length && (
                            <span className="source-chip source-chip-preview">
                              30s preview
                            </span>
                          )}
                          {owned && (
                            <span className="source-chip source-chip-owned">
                              <BiCheck /> In your library
                            </span>
                          )}
                          {track.attribution && (
                            <span className="source-chip source-chip-licence">
                              {track.attribution}
                            </span>
                          )}
                        </div>
                      </div>

                      <div className="source-hit-duration">
                        {state === "stream" ? (
                          <StreamProgress stage={stages[key]} />
                        ) : (
                          formatTime(track.duration_seconds)
                        )}
                      </div>
                    </button>

                    {track.downloadable && !owned && (
                      <button
                        type="button"
                        className="source-hit-download"
                        disabled={!!state}
                        onClick={() => onDownload(track)}
                        title="Save to your library"
                        aria-label={`Save ${track.title} to your library`}
                      >
                        {state === "download" ? (
                          <span className="import-spinner" />
                        ) : (
                          <BiCloudDownload />
                        )}
                      </button>
                    )}
                    <button
                      type="button"
                      className="source-hit-menu"
                      title="More"
                      aria-label={`More for ${track.title}`}
                      onClick={(event) => {
                        const rect =
                          event.currentTarget.getBoundingClientRect();
                        onOpenMenu(track, {
                          top: rect.bottom + 4,
                          flipAbove: rect.top - 4,
                          right: window.innerWidth - rect.right,
                        });
                      }}
                    >
                      <BiDotsHorizontalRounded />
                    </button>
                  </div>
                );
              })}
            </div>
          )}
        </section>
      ))}
    </div>
  );
}
