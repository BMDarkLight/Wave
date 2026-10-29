/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  BiExpandAlt,
  BiPause,
  BiPlay,
  BiSkipNext,
  BiSkipPrevious,
  BiX,
} from "react-icons/bi";
import Artwork from "./Artwork";
import { useTheme } from "../hooks/useTheme";
import { formatTime } from "../utils/format";
import {
  closeMiniPlayer,
  expandMiniPlayer,
  getPlaybackState,
  getQueueTracks,
  listenToQueueChanged,
  pauseTrack,
  playNext,
  playPrevious,
  playTrack,
  playTrackFromQueue,
  resumeTrack,
  seekTrack,
  type PlaybackState,
  type QueueTrackState,
} from "../utils/player";
import { emptyPlaybackState, getTrackTitle } from "../utils/track";
import "../MiniPlayer.css";

const POLL_MS = 500;
/** Seconds an arrow key moves the playhead. */
const SEEK_STEP = 5;

/**
 * The mini player window. It runs its own copy of the frontend and follows
 * playback by polling, like the main window does.
 */
export default function MiniPlayer() {
  useTheme(() => {});
  const [playback, setPlayback] = useState<PlaybackState>(emptyPlaybackState);
  const [queue, setQueue] = useState<QueueTrackState | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  /** Where the seek bar is held while dragging, in seconds. */
  const [scrub, setScrub] = useState<number | null>(null);
  const lastPath = useRef<string | null>(null);

  const refreshQueue = useCallback(() => {
    getQueueTracks()
      .then(setQueue)
      .catch(() => {});
  }, []);

  useEffect(() => {
    let alive = true;
    const tick = () =>
      getPlaybackState()
        .then((state) => {
          if (!alive) return;
          setPlayback(state);
          if (state.current_path !== lastPath.current) {
            lastPath.current = state.current_path;
            refreshQueue();
          }
        })
        .catch(() => {});
    tick();
    const timer = window.setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [refreshQueue]);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    listenToQueueChanged(refreshQueue)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [refreshQueue]);

  const track =
    queue && queue.current_index !== null
      ? (queue.tracks[queue.current_index] ?? null)
      : null;
  const duration = playback.duration_seconds ?? track?.duration_seconds ?? 0;
  const position = scrub ?? playback.position_seconds;

  const run = useCallback(async (action: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await action();
      setPlayback(await getPlaybackState());
    } catch (err) {
      setProblem(String(err));
    }
  }, []);

  const togglePlay = useCallback(
    () =>
      run(async () => {
        if (playback.is_playing) await pauseTrack();
        else if (playback.is_paused) await resumeTrack();
        else if (playback.current_path) await playTrack(playback.current_path);
        else if (queue?.tracks.length)
          await playTrackFromQueue(queue.current_index ?? 0);
      }),
    [playback, queue, run],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === " ") {
        event.preventDefault();
        void togglePlay();
      } else if (event.key === "ArrowRight" && duration > 0) {
        void run(() =>
          seekTrack(Math.min(duration, playback.position_seconds + SEEK_STEP)),
        );
      } else if (event.key === "ArrowLeft") {
        void run(() =>
          seekTrack(Math.max(0, playback.position_seconds - SEEK_STEP)),
        );
      } else if (event.key === "Escape") {
        void closeMiniPlayer();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [togglePlay, run, duration, playback.position_seconds]);

  const title = track
    ? getTrackTitle(track)
    : playback.current_path
      ? (playback.current_path.split(/[\\/]/).pop() ?? "")
      : "Nothing playing";
  const subtitle = problem ?? (track ? track.artist : "Open a song to play it");

  return (
    <div className="mini-player" data-tauri-drag-region>
      <Artwork
        track={track}
        fallback="♪"
        className="mini-art"
        alt={track ? `${title} cover` : "No cover"}
      />
      <div className="mini-body" data-tauri-drag-region>
        <div className="mini-top" data-tauri-drag-region>
          <div className="mini-text" data-tauri-drag-region>
            <span className="mini-title" title={title} data-tauri-drag-region>
              {title}
            </span>
            <span
              className={`mini-subtitle${problem ? " mini-problem" : ""}`}
              title={subtitle}
              data-tauri-drag-region
            >
              {subtitle}
            </span>
          </div>
          <button
            type="button"
            className="mini-window-btn"
            aria-label="Open the full player"
            title="Open the full player"
            onClick={() => void expandMiniPlayer()}
          >
            <BiExpandAlt />
          </button>
          <button
            type="button"
            className="mini-window-btn"
            aria-label="Close"
            title="Close"
            onClick={() => void closeMiniPlayer()}
          >
            <BiX />
          </button>
        </div>
        <div className="mini-controls" data-tauri-drag-region>
          <button
            type="button"
            className="mini-btn"
            aria-label="Previous"
            onClick={() => void run(playPrevious)}
          >
            <BiSkipPrevious />
          </button>
          <button
            type="button"
            className="mini-btn mini-play"
            aria-label={playback.is_playing ? "Pause" : "Play"}
            onClick={() => void togglePlay()}
          >
            {playback.is_playing ? <BiPause /> : <BiPlay />}
          </button>
          <button
            type="button"
            className="mini-btn"
            aria-label="Next"
            onClick={() => void run(playNext)}
          >
            <BiSkipNext />
          </button>
          <span className="mini-time" data-tauri-drag-region>
            {formatTime(position)} / {formatTime(duration)}
          </span>
        </div>
      </div>
      <input
        className="mini-seek"
        type="range"
        min={0}
        max={duration || 1}
        step={0.1}
        value={Math.min(position, duration || 1)}
        disabled={!duration}
        aria-label="Seek"
        style={{
          backgroundSize: `${duration ? (position / duration) * 100 : 0}% 100%`,
        }}
        onChange={(event) => setScrub(Number(event.target.value))}
        onPointerUp={() => {
          if (scrub === null) return;
          const to = scrub;
          void run(() => seekTrack(to)).finally(() => setScrub(null));
        }}
        onKeyUp={() => {
          if (scrub === null) return;
          const to = scrub;
          void run(() => seekTrack(to)).finally(() => setScrub(null));
        }}
      />
    </div>
  );
}
