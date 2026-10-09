/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  getPlaybackLevel,
  getWaveform,
  listenToWaveformReady,
} from "../utils/player";
import {
  REST_HEIGHT,
  barHeights,
  liveLift,
  smoothLevel,
} from "../utils/waveform";
import { lessMotion } from "../utils/appearance";

/** Height of a bar at rest before it grows, matching the plain slider. */
const MIN_BAR = 2;

/** How long the bars take to grow in when a waveform appears. */
const ENTRANCE_MS = 560;
/** Delay from the first bar to the last, so they grow in left to right. */
const ENTRANCE_SWEEP_MS = 240;

/**
 * How far a bar has grown from a flat line to its full height, 0 to 1,
 * `shownFor` ms after the waveform appeared. `along` is the bar's position
 * from 0 (left) to 1 (right).
 */
function entranceProgress(shownFor: number, along: number): number {
  const own = ENTRANCE_MS - ENTRANCE_SWEEP_MS;
  const t = Math.min(
    1,
    Math.max(0, (shownFor - along * ENTRANCE_SWEEP_MS) / own),
  );
  return 1 - (1 - t) ** 3;
}

/** The stored waveform for `path`, or `null` until it is ready. */
function useWaveform(path: string | null): number[] | null {
  const [loaded, setLoaded] = useState<{
    path: string;
    bins: number[] | null;
  } | null>(null);

  useEffect(() => {
    if (!path) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    const load = () =>
      getWaveform(path)
        .then((bins) => {
          if (!cancelled) setLoaded({ path, bins });
        })
        .catch(() => {});
    load();
    listenToWaveformReady((ready) => {
      if (ready === path) load();
    })
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [path]);

  return loaded && loaded.path === path ? loaded.bins : null;
}

/**
 * Seek bar drawn as the track's waveform. The part already played is lit,
 * and while playing the bars around the playhead move with the music.
 * Shows `fallback` (the plain slider) until the waveform is ready, and for
 * tracks that have none, such as streams.
 */
export default function WaveformSeek({
  path,
  position,
  duration,
  playing,
  speed,
  compact = false,
  onSeekChange,
  onSeekCommit,
  fallback,
}: {
  path: string | null;
  position: number;
  duration: number;
  playing: boolean;
  speed: number;
  /** Thinner bars, for the desktop player bar. */
  compact?: boolean;
  onSeekChange: (seconds: number) => void;
  onSeekCommit: (seconds: number) => void;
  fallback: ReactNode;
}) {
  const bins = useWaveform(path);
  if (!bins || duration <= 0) return <>{fallback}</>;
  return (
    <WaveformCanvas
      key={path}
      bins={bins}
      position={position}
      duration={duration}
      playing={playing}
      speed={speed}
      compact={compact}
      onSeekChange={onSeekChange}
      onSeekCommit={onSeekCommit}
    />
  );
}

function WaveformCanvas({
  bins,
  position,
  duration,
  playing,
  speed,
  compact,
  onSeekChange,
  onSeekCommit,
}: {
  bins: number[];
  position: number;
  duration: number;
  playing: boolean;
  speed: number;
  compact: boolean;
  onSeekChange: (seconds: number) => void;
  onSeekCommit: (seconds: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dragging = useRef(false);
  const measuredLevel = useRef(0);
  const shownLevel = useRef(0);
  // When the waveform first appeared, for the entrance animation.
  const shownAt = useRef(0);
  // Where the mouse is hovering, as a fraction of the width.
  const hover = useRef<number | null>(null);
  // Redraw once; set by the drawing effect.
  const redraw = useRef<() => void>(() => {});
  // What the drawing loop reads. `at` is when `position` was last reported,
  // so the playhead can move smoothly between the half-second polls.
  const frame = useRef({ bins, position, duration, playing, speed, at: 0 });

  useEffect(() => {
    frame.current = {
      bins,
      position,
      duration,
      playing,
      speed,
      at: performance.now(),
    };
  }, [bins, position, duration, playing, speed]);

  // Poll the live level about 30 times a second, only while playing.
  useEffect(() => {
    if (!playing) {
      measuredLevel.current = 0;
      return;
    }
    let stopped = false;
    let waiting = false;
    const timer = window.setInterval(() => {
      if (waiting) return;
      waiting = true;
      getPlaybackLevel()
        .then((level) => {
          if (!stopped) measuredLevel.current = level;
        })
        .catch(() => {})
        .finally(() => {
          waiting = false;
        });
    }, 33);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [playing]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let raf = 0;

    const draw = () => {
      const context = canvas.getContext("2d");
      if (!context) return;
      const width = canvas.clientWidth;
      const height = canvas.clientHeight;
      const ratio = window.devicePixelRatio || 1;
      if (canvas.width !== Math.round(width * ratio)) {
        canvas.width = Math.round(width * ratio);
      }
      if (canvas.height !== Math.round(height * ratio)) {
        canvas.height = Math.round(height * ratio);
      }
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);

      const now = frame.current;
      let seconds = now.position;
      if (now.playing && !document.body.classList.contains("is-seeking")) {
        seconds += ((performance.now() - now.at) / 1000) * now.speed;
      }
      const fraction =
        now.duration > 0 ? Math.min(1, Math.max(0, seconds / now.duration)) : 0;

      shownLevel.current = smoothLevel(
        shownLevel.current,
        now.playing ? measuredLevel.current : 0,
      );

      const style = getComputedStyle(canvas);
      const playedColor =
        style.getPropertyValue("--waveform-played").trim() || "#fff";
      const restColor =
        style.getPropertyValue("--waveform-rest").trim() ||
        "rgba(255, 255, 255, 0.25)";
      const hoverColor =
        style.getPropertyValue("--waveform-hover").trim() ||
        "rgba(255, 255, 255, 0.5)";
      const bar = 2;
      const gap = compact ? 1 : 2;
      const count = Math.max(1, Math.floor((width + gap) / (bar + gap)));
      const heights = barHeights(now.bins, count);
      const head = fraction * count;
      const hoverHead = hover.current === null ? null : hover.current * count;
      const shownFor = lessMotion()
        ? ENTRANCE_MS
        : performance.now() - shownAt.current;
      const round = typeof context.roundRect === "function";

      for (let i = 0; i < count; i++) {
        const lift = liveLift(shownLevel.current, Math.abs(i + 0.5 - head));
        const full = (heights[i] / 255) * height * REST_HEIGHT * lift;
        const grown = entranceProgress(shownFor, i / count);
        const barHeight = Math.min(
          height,
          Math.max(MIN_BAR, MIN_BAR + (full - MIN_BAR) * grown),
        );
        const centre = i + 0.5;
        context.fillStyle =
          centre < head
            ? playedColor
            : hoverHead !== null && centre < hoverHead
              ? hoverColor
              : restColor;
        const x = i * (bar + gap);
        const y = (height - barHeight) / 2;
        if (round) {
          context.beginPath();
          context.roundRect(x, y, bar, barHeight, bar / 2);
          context.fill();
        } else {
          context.fillRect(x, y, bar, barHeight);
        }
      }
    };

    const loop = () => {
      draw();
      const settling = shownLevel.current > 0.001;
      const entering =
        !lessMotion() && performance.now() - shownAt.current < ENTRANCE_MS;
      if (frame.current.playing || settling || entering) {
        raf = requestAnimationFrame(loop);
      }
    };
    if (shownAt.current === 0) shownAt.current = performance.now();
    loop();
    redraw.current = () => {
      cancelAnimationFrame(raf);
      loop();
    };

    const resize = new ResizeObserver(() => draw());
    resize.observe(canvas);
    return () => {
      cancelAnimationFrame(raf);
      resize.disconnect();
    };
  }, [bins, position, duration, playing, speed, compact]);

  const secondsAt = (clientX: number) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return 0;
    const fraction = Math.min(
      1,
      Math.max(0, (clientX - rect.left) / rect.width),
    );
    return fraction * duration;
  };

  return (
    <canvas
      ref={canvasRef}
      className={`waveform-seek${compact ? " waveform-seek-compact" : ""}`}
      data-no-drag-dismiss
      // The arrow keys already seek from anywhere in the app.
      role="slider"
      tabIndex={0}
      aria-label="Seek"
      aria-valuemin={0}
      aria-valuemax={Math.round(duration)}
      aria-valuenow={Math.round(position)}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        dragging.current = true;
        document.body.classList.add("is-seeking");
        onSeekChange(secondsAt(event.clientX));
      }}
      onPointerMove={(event) => {
        if (dragging.current) {
          onSeekChange(secondsAt(event.clientX));
          return;
        }
        if (event.pointerType !== "mouse" || duration <= 0) return;
        hover.current = secondsAt(event.clientX) / duration;
        redraw.current();
      }}
      onPointerLeave={() => {
        hover.current = null;
        redraw.current();
      }}
      onPointerUp={(event) => {
        if (!dragging.current) return;
        dragging.current = false;
        onSeekCommit(secondsAt(event.clientX));
      }}
      onPointerCancel={() => {
        dragging.current = false;
        document.body.classList.remove("is-seeking");
      }}
    />
  );
}
