// Mobile-only fullscreen "Now Playing" page. Replaces the desktop lyrics
// sidebar on narrow/responsive layouts: big cover art, transport controls,
// a lyrics view toggle, and a bottom-sheet menu with the volume dial + EQ.
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BiChevronDown,
  BiHeart,
  BiSolidHeart,
  BiShuffle,
  BiSkipPrevious,
  BiSkipNext,
  BiPlay,
  BiPause,
  BiRepeat,
  BiAlignLeft,
  BiListUl,
  BiSliderAlt,
  BiX,
  BiGridVertical,
  BiVolumeMute,
  BiVolumeLow,
  BiVolumeFull,
} from "react-icons/bi";
import {
  getTrackFullCover,
  getTrackDetails,
  resolveCoverSrc,
  EQ_BAND_LABELS,
  EQ_PRESETS,
} from "../utils/player";
import type { Track, PlaybackMode, EqSettings } from "../utils/player";
import { useDragDismiss } from "../hooks/useDragDismiss";

const formatTime = (seconds?: number | null) => {
  if (!seconds || !Number.isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60)
    .toString()
    .padStart(2, "0");
  return `${m}:${s}`;
};

const getTrackTitle = (track?: Track | null) => {
  if (track?.title) return track.title;
  if (track?.name) return track.name;
  return "Unknown";
};

type LyricLine = { time: number; text: string };
const LRC_TAG_RE = /\[(\d{1,2}):(\d{2})(?:\.(\d{1,3}))?\]/g;

const parseTimedLyrics = (raw?: string | null): LyricLine[] | null => {
  if (!raw) return null;
  const lines = raw.split(/\r?\n/);
  const result: LyricLine[] = [];
  let matchedLines = 0;
  let nonEmptyLines = 0;

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    nonEmptyLines++;
    const tags = [...trimmed.matchAll(LRC_TAG_RE)];
    if (tags.length === 0) continue;
    matchedLines++;
    const text = trimmed.replace(LRC_TAG_RE, "").trim();
    for (const tag of tags) {
      const minutes = parseInt(tag[1], 10);
      const seconds = parseInt(tag[2], 10);
      const fraction = tag[3] ? parseFloat(`0.${tag[3]}`) : 0;
      result.push({ time: minutes * 60 + seconds + fraction, text });
    }
  }

  if (nonEmptyLines === 0 || matchedLines < nonEmptyLines * 0.4) return null;
  result.sort((a, b) => a.time - b.time);
  return result;
};

const Artwork = ({
  track,
  overrideSrc,
  fallback,
  className,
}: {
  track: Track;
  overrideSrc?: string | null;
  fallback: string;
  className: string;
}) => {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const raw = overrideSrc || track.cover_art_data_url || null;
    void resolveCoverSrc(raw).then((resolved) => {
      if (!cancelled) setSrc(resolved);
    });
    return () => {
      cancelled = true;
    };
  }, [track.cover_art_data_url, overrideSrc]);

  if (src) {
    return (
      <img
        className={className}
        src={src}
        alt={`${getTrackTitle(track)} cover`}
        draggable={false}
      />
    );
  }
  return <div className={className}>{fallback}</div>;
};

/** Circular drag dial (270° sweep, gap at the bottom). `value` is 0..1. */
function CircularDial({
  value,
  onChange,
  size = 128,
  ariaLabel,
  ariaValueMin = 0,
  ariaValueMax = 100,
  ariaValueNow,
  formatCenter,
  className = "",
}: {
  value: number;
  onChange: (value: number) => void;
  size?: number;
  ariaLabel: string;
  ariaValueMin?: number;
  ariaValueMax?: number;
  ariaValueNow: number;
  formatCenter: (value: number) => ReactNode;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const r = size * 0.36;
  const stroke = Math.max(7, size * 0.078);
  const thumb = Math.max(5, size * 0.055);
  const cx = size / 2;
  const cy = size / 2;
  const clamped = Math.max(0, Math.min(1, value));

  const updateFromPointer = (clientX: number, clientY: number) => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const dx = clientX - centerX;
    const dy = clientY - centerY;
    const deg = Math.atan2(dy, dx) * (180 / Math.PI);
    let a = deg + 90;
    a = ((a % 360) + 360) % 360;
    let shifted = a - 225;
    if (shifted < 0) shifted += 360;
    let next: number;
    if (shifted <= 270) {
      next = shifted / 270;
    } else {
      next = shifted < 315 ? 1 : 0;
    }
    onChange(Math.max(0, Math.min(1, next)));
  };

  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: PointerEvent) => updateFromPointer(e.clientX, e.clientY);
    const onUp = () => setDragging(false);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dragging]);

  const angleFor = (v: number) => 225 + Math.max(0, Math.min(1, v)) * 270;
  const pointFor = (angleDeg: number) => {
    const rad = (angleDeg * Math.PI) / 180;
    return { x: cx + r * Math.sin(rad), y: cy - r * Math.cos(rad) };
  };

  const startPoint = pointFor(225);
  const endPoint = pointFor(135);
  const valuePoint = pointFor(angleFor(clamped));
  const valueSweepDeg = clamped * 270;

  return (
    <div
      ref={ref}
      className={`mnp-dial ${dragging ? "dragging" : ""} ${className}`.trim()}
      role="slider"
      tabIndex={0}
      aria-label={ariaLabel}
      aria-valuemin={ariaValueMin}
      aria-valuemax={ariaValueMax}
      aria-valuenow={ariaValueNow}
      onPointerDown={(e) => {
        e.preventDefault();
        setDragging(true);
        updateFromPointer(e.clientX, e.clientY);
      }}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp" || e.key === "ArrowRight") {
          e.preventDefault();
          onChange(Math.min(1, clamped + 0.05));
        } else if (e.key === "ArrowDown" || e.key === "ArrowLeft") {
          e.preventDefault();
          onChange(Math.max(0, clamped - 0.05));
        }
      }}
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <path
          d={`M ${startPoint.x} ${startPoint.y} A ${r} ${r} 0 1 1 ${endPoint.x} ${endPoint.y}`}
          fill="none"
          stroke="rgba(255,255,255,0.15)"
          strokeWidth={stroke}
          strokeLinecap="round"
        />
        {clamped > 0 && (
          <path
            d={`M ${startPoint.x} ${startPoint.y} A ${r} ${r} 0 ${valueSweepDeg > 180 ? 1 : 0} 1 ${valuePoint.x} ${valuePoint.y}`}
            fill="none"
            stroke="#fff"
            strokeWidth={stroke}
            strokeLinecap="round"
          />
        )}
        <circle cx={valuePoint.x} cy={valuePoint.y} r={thumb} fill="#fff" />
      </svg>
      <div className="mnp-dial-center">{formatCenter(clamped)}</div>
    </div>
  );
}

function VolumeDial({
  value,
  onChange,
  size = 104,
}: {
  value: number;
  onChange: (value: number) => void;
  size?: number;
}) {
  const VolumeIcon =
    value === 0 ? BiVolumeMute : value < 0.5 ? BiVolumeLow : BiVolumeFull;
  return (
    <CircularDial
      value={value}
      onChange={onChange}
      size={size}
      ariaLabel="Volume"
      ariaValueNow={Math.round(value * 100)}
      formatCenter={() => (
        <>
          <VolumeIcon />
          <span>{Math.round(value * 100)}%</span>
        </>
      )}
    />
  );
}

const EQ_GAIN_MIN = -12;
const EQ_GAIN_MAX = 12;
const BASS_BAND_INDEXES = [0, 1, 2] as const; // 31 / 62 / 125 Hz
const TREBLE_BAND_INDEXES = [7, 8, 9] as const; // 4k / 8k / 16k Hz

const gainToDial = (gain: number) =>
  (Math.max(EQ_GAIN_MIN, Math.min(EQ_GAIN_MAX, gain)) - EQ_GAIN_MIN) /
  (EQ_GAIN_MAX - EQ_GAIN_MIN);

const dialToGain = (value: number) => {
  const raw = value * (EQ_GAIN_MAX - EQ_GAIN_MIN) + EQ_GAIN_MIN;
  return Math.round(raw * 2) / 2;
};

const formatGain = (gain: number) =>
  `${gain > 0 ? "+" : ""}${gain.toFixed(gain % 1 === 0 ? 0 : 1)}`;

const averageBandGain = (bands: number[], indexes: readonly number[]) => {
  if (indexes.length === 0) return 0;
  return indexes.reduce((sum, i) => sum + (bands[i] ?? 0), 0) / indexes.length;
};

function ToneDial({
  label,
  gain,
  onChange,
  size = 104,
}: {
  label: string;
  gain: number;
  onChange: (gain: number) => void;
  size?: number;
}) {
  return (
    <CircularDial
      value={gainToDial(gain)}
      onChange={(v) => onChange(dialToGain(v))}
      size={size}
      ariaLabel={label}
      ariaValueMin={EQ_GAIN_MIN}
      ariaValueMax={EQ_GAIN_MAX}
      ariaValueNow={Math.round(gain)}
      className="mnp-dial-tone"
      formatCenter={() => (
        <>
          <span className="mnp-dial-label">{label}</span>
          <span>{formatGain(gain)} dB</span>
        </>
      )}
    />
  );
}

export type MobileNowPlayingView = "cover" | "lyrics" | "queue";

interface MobileNowPlayingProps {
  track: Track;
  isPlaying: boolean;
  isFavorite: boolean;
  onToggleFavorite: () => void;
  displayPosition: number;
  displayDuration: number;
  onSeekChange: (value: number) => void;
  onSeekCommit: (value: number) => void;
  playbackMode: PlaybackMode;
  canSkip: boolean;
  onPlayPause: () => void;
  onPrevious: () => void;
  onNext: () => void;
  onToggleShuffle: () => void;
  onCycleRepeat: () => void;
  closing?: boolean;
  onClose: () => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  volumeValue: number;
  onVolumeChange: (value: number) => void;
  eqSettings: EqSettings;
  onEqEnabledChange: (enabled: boolean) => void;
  onEqBandChange: (index: number, gain: number) => void;
  onEqBandsChange: (bands: number[]) => void;
  onEqPreset: (id: string) => void;
  onEqReset: () => void;
  view: MobileNowPlayingView;
  onViewChange: (view: MobileNowPlayingView) => void;
  menuOpen: boolean;
  onMenuOpenChange: (open: boolean) => void;
  queueTracks: Track[];
  queueCurrentIndex: number | null;
  onPlayFromQueue: (index: number) => void;
  onRemoveFromQueue: (index: number) => void;
  onReorderQueue: (from: number, to: number) => void;
  onClearQueue: () => void;
}

export default function MobileNowPlaying({
  track,
  isPlaying,
  isFavorite,
  onToggleFavorite,
  displayPosition,
  displayDuration,
  onSeekChange,
  onSeekCommit,
  playbackMode,
  canSkip,
  onPlayPause,
  onPrevious,
  onNext,
  onToggleShuffle,
  onCycleRepeat,
  closing = false,
  onClose,
  onOpenArtist,
  onOpenAlbum,
  volumeValue,
  onVolumeChange,
  eqSettings,
  onEqEnabledChange,
  onEqBandChange,
  onEqBandsChange,
  onEqPreset,
  onEqReset,
  view,
  onViewChange,
  menuOpen,
  onMenuOpenChange,
  queueTracks,
  queueCurrentIndex,
  onPlayFromQueue,
  onRemoveFromQueue,
  onReorderQueue,
  onClearQueue,
}: MobileNowPlayingProps) {
  const [fullCover, setFullCover] = useState<string | null>(null);
  const [entered, setEntered] = useState(false);
  const [lyricsText, setLyricsText] = useState<string | null>(
    track.lyrics ?? null,
  );
  const [lyricsSource, setLyricsSource] = useState<string | null>(
    track.lyrics_source ?? null,
  );
  const activeLineRef = useRef<HTMLButtonElement>(null);
  const queueListRef = useRef<HTMLDivElement>(null);
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [overIndex, setOverIndex] = useState<number | null>(null);
  const dragIndexRef = useRef<number | null>(null);
  const dragMovedRef = useRef(false);
  const [sheetMounted, setSheetMounted] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);

  const closeSheet = () => onMenuOpenChange(false);

  /** Step back through sheet → cover → dismiss, matching Android back. */
  const handleHeaderBack = () => {
    if (menuOpen) {
      closeSheet();
      return;
    }
    if (view !== "cover") {
      onViewChange("cover");
      return;
    }
    onClose();
  };

  const pageDismiss = useDragDismiss({
    onDismiss: onClose,
    enabled: !closing,
  });

  const sheetDismiss = useDragDismiss({
    onDismiss: closeSheet,
    enabled: sheetOpen && !closing,
    threshold: 90,
  });

  useEffect(() => {
    const id = requestAnimationFrame(() => {
      requestAnimationFrame(() => setEntered(true));
    });
    return () => cancelAnimationFrame(id);
  }, []);

  useEffect(() => {
    if (menuOpen) {
      setSheetMounted(true);
      const id = requestAnimationFrame(() => {
        requestAnimationFrame(() => setSheetOpen(true));
      });
      return () => cancelAnimationFrame(id);
    }
    setSheetOpen(false);
    const timer = window.setTimeout(() => setSheetMounted(false), 300);
    return () => window.clearTimeout(timer);
  }, [menuOpen]);

  useEffect(() => {
    let cancelled = false;
    setFullCover(null);
    setLyricsText(track.lyrics ?? null);
    setLyricsSource(track.lyrics_source ?? null);
    if (!track.path) return;
    void Promise.all([
      getTrackFullCover(track.path),
      getTrackDetails(track.path),
    ]).then(([cover, details]) => {
      if (cancelled) return;
      if (cover) setFullCover(cover);
      if (details?.lyrics) {
        setLyricsText(details.lyrics);
        setLyricsSource(details.lyrics_source ?? null);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [track.path]);

  const timedLyrics = useMemo(() => parseTimedLyrics(lyricsText), [lyricsText]);

  const activeLyricIndex = useMemo(() => {
    if (!timedLyrics) return -1;
    let idx = -1;
    for (let i = 0; i < timedLyrics.length; i++) {
      if (timedLyrics[i].time <= displayPosition + 0.15) idx = i;
      else break;
    }
    return idx;
  }, [timedLyrics, displayPosition]);

  useEffect(() => {
    if (view === "lyrics" && activeLineRef.current) {
      activeLineRef.current.scrollIntoView({
        block: "center",
        behavior: "smooth",
      });
    }
  }, [activeLyricIndex, view]);

  const title = getTrackTitle(track);
  const coverLetters = title.slice(0, 2).toUpperCase();

  const resolveQueueDropIndex = (clientY: number) => {
    const list = queueListRef.current;
    if (!list) return null;
    const items = [
      ...list.querySelectorAll<HTMLElement>("[data-queue-index]"),
    ];
    if (items.length === 0) return null;
    let best = 0;
    let bestDist = Number.POSITIVE_INFINITY;
    for (const el of items) {
      const rect = el.getBoundingClientRect();
      const mid = rect.top + rect.height / 2;
      const dist = Math.abs(clientY - mid);
      if (dist < bestDist) {
        bestDist = dist;
        best = Number(el.dataset.queueIndex);
      }
    }
    return best;
  };

  const endQueueDrag = (clientY?: number) => {
    const from = dragIndexRef.current;
    if (from == null) return;
    const target =
      clientY != null ? resolveQueueDropIndex(clientY) : overIndex;
    dragIndexRef.current = null;
    setDragIndex(null);
    setOverIndex(null);
    if (
      target != null &&
      from !== target &&
      target >= 0 &&
      target < queueTracks.length
    ) {
      onReorderQueue(from, target);
    }
  };

  const bassGain = averageBandGain(eqSettings.bands, BASS_BAND_INDEXES);
  const trebleGain = averageBandGain(eqSettings.bands, TREBLE_BAND_INDEXES);

  const applyToneGain = (indexes: readonly number[], gain: number) => {
    const next = eqSettings.bands.map((value, index) =>
      indexes.includes(index) ? gain : value,
    );
    onEqBandsChange(next);
  };

  const pageDragStyle =
    !closing && (pageDismiss.dragging || pageDismiss.offset > 0)
      ? {
          transform: `translateY(${pageDismiss.offset}px)`,
          opacity: Math.max(0.4, 1 - pageDismiss.offset / 520),
        }
      : undefined;

  const sheetDragStyle =
    sheetOpen && (sheetDismiss.dragging || sheetDismiss.offset > 0)
      ? { transform: `translateY(${sheetDismiss.offset}px)` }
      : undefined;

  return (
    <div
      className={`mobile-now-playing${entered && !closing ? " mnp-open" : ""}${closing ? " mnp-closing" : ""}${view !== "cover" ? " mnp-expanded" : ""}${pageDismiss.dragging ? " mnp-dragging" : ""}`}
      style={pageDragStyle}
    >
      <div className="mnp-header" {...pageDismiss.bind}>
        <button
          className="mnp-icon-btn"
          onClick={handleHeaderBack}
          type="button"
          aria-label={
            menuOpen || view !== "cover" ? "Go back" : "Minimize player"
          }
        >
          <BiChevronDown />
        </button>
        <span className="mnp-header-label">Now Playing</span>
        <button
          className={`mnp-icon-btn ${isFavorite ? "active" : ""}`}
          onClick={onToggleFavorite}
          type="button"
          aria-label={isFavorite ? "Remove from Favorites" : "Add to Favorites"}
        >
          {isFavorite ? <BiSolidHeart /> : <BiHeart />}
        </button>
      </div>

      <div className="mnp-body">
        <div
          className={`mnp-layer mnp-cover-wrap ${view === "cover" ? "mnp-layer-active" : ""}`}
          {...(view === "cover" && !menuOpen ? pageDismiss.bind : {})}
        >
          <Artwork
            track={track}
            overrideSrc={fullCover}
            fallback={coverLetters}
            className="mnp-cover"
          />
        </div>

        <div
          className={`mnp-layer mnp-lyrics-scroll ${view === "lyrics" ? "mnp-layer-active" : ""}`}
        >
          {timedLyrics ? (
            <div className="lyrics-lines">
              {timedLyrics.map((line, index) => (
                <button
                  key={`${line.time}-${index}`}
                  ref={index === activeLyricIndex ? activeLineRef : null}
                  type="button"
                  className={`lyrics-line ${index === activeLyricIndex ? "active" : ""}`}
                  onClick={() => onSeekCommit(line.time)}
                >
                  {line.text || "\u00A0"}
                </button>
              ))}
            </div>
          ) : lyricsText ? (
            <pre>{lyricsText}</pre>
          ) : (
            <p className="lyrics-empty">No lyrics available</p>
          )}
          {lyricsText && (
            <p className="lyrics-source">
              {lyricsSource === "lrclib"
                ? "Lyrics provided by LRCLIB"
                : "Lyrics pulled from the file"}
            </p>
          )}
        </div>

        <div
          className={`mnp-layer mnp-queue-scroll ${view === "queue" ? "mnp-layer-active" : ""}`}
        >
          <div className="mnp-queue-header">
            <span>Up Next</span>
            {queueTracks.length > 0 && (
              <button
                className="btn-ghost btn-sm"
                onClick={onClearQueue}
                type="button"
              >
                Clear
              </button>
            )}
          </div>
          {queueTracks.length === 0 ? (
            <div className="queue-empty">
              <p>Queue is empty</p>
              <span>Add tracks with "Play Next" or "Add to Queue"</span>
            </div>
          ) : (
            <div
              className={`mnp-queue-list${dragIndex != null ? " is-reordering" : ""}`}
              ref={queueListRef}
            >
              {queueTracks.map((qTrack, index) => (
                <div
                  key={`${qTrack.path}-${index}`}
                  data-queue-index={index}
                  className={`queue-item mnp-queue-item${queueCurrentIndex === index ? " active" : ""}${dragIndex === index ? " is-dragging" : ""}${overIndex === index && dragIndex != null && dragIndex !== index ? " drop-target" : ""}`}
                  onClick={() => {
                    if (dragMovedRef.current) {
                      dragMovedRef.current = false;
                      return;
                    }
                    onPlayFromQueue(index);
                  }}
                >
                  <button
                    className="mnp-queue-handle"
                    type="button"
                    title="Drag to reorder"
                    aria-label="Drag to reorder"
                    onClick={(e) => e.stopPropagation()}
                    onPointerDown={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      dragMovedRef.current = false;
                      dragIndexRef.current = index;
                      setDragIndex(index);
                      setOverIndex(index);
                      e.currentTarget.setPointerCapture(e.pointerId);
                    }}
                    onPointerMove={(e) => {
                      if (dragIndexRef.current == null) return;
                      dragMovedRef.current = true;
                      const next = resolveQueueDropIndex(e.clientY);
                      if (next != null) setOverIndex(next);
                    }}
                    onPointerUp={(e) => {
                      e.stopPropagation();
                      endQueueDrag(e.clientY);
                    }}
                    onPointerCancel={() => endQueueDrag()}
                  >
                    <BiGridVertical />
                  </button>
                  <Artwork
                    track={qTrack}
                    fallback={getTrackTitle(qTrack).slice(0, 1).toUpperCase()}
                    className="queue-thumb"
                  />
                  <div className="queue-item-info">
                    <div className="queue-item-name">{getTrackTitle(qTrack)}</div>
                    <div className="queue-item-artist">{qTrack.artist}</div>
                  </div>
                  <div className="queue-item-actions">
                    <button
                      className="queue-item-remove"
                      onClick={(e) => {
                        e.stopPropagation();
                        onRemoveFromQueue(index);
                      }}
                      title="Remove from queue"
                      type="button"
                    >
                      <BiX />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="mnp-meta">
        <div className="mnp-title" title={title}>
          {title}
        </div>
        <div className="mnp-meta-row">
          <button
            className="mnp-artist"
            onClick={() => track.artist && onOpenArtist(track.artist)}
            type="button"
            disabled={!track.artist}
          >
            {track.artist || "Unknown artist"}
          </button>
          {track.album && (
            <button
              className="mnp-album"
              onClick={() =>
                onOpenAlbum(track.album, track.album_artist || track.artist)
              }
              type="button"
            >
              {track.album}
            </button>
          )}
        </div>
      </div>

      <div className="mnp-seek-row">
        <input
          className="range-slider"
          type="range"
          min="0"
          max={Math.max(displayDuration, 1)}
          step="1"
          value={displayPosition}
          onPointerDown={() => document.body.classList.add("is-seeking")}
          onChange={(e) => onSeekChange(Number(e.target.value))}
          onPointerUp={(e) => onSeekCommit(Number(e.currentTarget.value))}
        />
        <div className="mnp-seek-times">
          <span>{formatTime(displayPosition)}</span>
          <span>{formatTime(displayDuration)}</span>
        </div>
      </div>

      <div className="mnp-controls">
        <button
          className={`control-btn shuffle-btn ${playbackMode.shuffle ? "active" : ""}`}
          onClick={onToggleShuffle}
          type="button"
          title="Shuffle"
        >
          <BiShuffle />
        </button>
        <button
          className="control-btn"
          onClick={onPrevious}
          disabled={!canSkip}
          type="button"
          title="Previous"
        >
          <BiSkipPrevious />
        </button>
        <button
          className="control-btn play-pause-btn mnp-play-btn"
          onClick={onPlayPause}
          type="button"
          title="Play/Pause"
        >
          {isPlaying ? <BiPause /> : <BiPlay />}
        </button>
        <button
          className="control-btn"
          onClick={onNext}
          disabled={!canSkip}
          type="button"
          title="Next"
        >
          <BiSkipNext />
        </button>
        <button
          className={`control-btn repeat-btn ${playbackMode.repeat !== "off" ? "active" : ""} ${playbackMode.repeat === "one" ? "repeat-one" : ""}`}
          onClick={onCycleRepeat}
          type="button"
          title="Repeat"
        >
          <BiRepeat />
        </button>
      </div>

      <div className="mnp-actions">
        <button
          className={`mnp-action-btn ${view === "lyrics" ? "active" : ""}`}
          onClick={() => onViewChange(view === "lyrics" ? "cover" : "lyrics")}
          type="button"
          title="Lyrics"
          aria-label="Toggle lyrics"
        >
          <BiAlignLeft />
        </button>
        <button
          className={`mnp-action-btn ${view === "queue" ? "active" : ""}`}
          onClick={() => onViewChange(view === "queue" ? "cover" : "queue")}
          type="button"
          title="Queue"
          aria-label="Toggle queue"
        >
          <BiListUl />
        </button>
        <button
          className={`mnp-action-btn ${menuOpen ? "active" : ""}`}
          onClick={() => onMenuOpenChange(true)}
          type="button"
          title="Volume & Equalizer"
          aria-label="Open volume and equalizer"
        >
          <BiSliderAlt />
        </button>
      </div>

      {sheetMounted && (
        <>
          <button
            className={`mnp-sheet-backdrop${sheetOpen ? " mnp-sheet-open" : ""}`}
            onClick={closeSheet}
            type="button"
            aria-label="Close menu"
          />
          <div
            className={`mnp-sheet${sheetOpen ? " mnp-sheet-open" : ""}${sheetDismiss.dragging ? " mnp-sheet-dragging" : ""}`}
            role="dialog"
            aria-label="Volume and equalizer"
            style={sheetDragStyle}
          >
            <div className="mnp-sheet-handle" {...sheetDismiss.bind} />
            <div className="mnp-sheet-header" {...sheetDismiss.bind}>
              <h3>Playback</h3>
              <button
                className="mnp-icon-btn"
                onClick={closeSheet}
                type="button"
                aria-label="Close"
              >
                <BiX />
              </button>
            </div>
            <div className="mnp-sheet-scroll">
              <div className="mnp-volume-section">
                <ToneDial
                  label="Bass"
                  gain={bassGain}
                  onChange={(gain) => applyToneGain(BASS_BAND_INDEXES, gain)}
                />
                <VolumeDial value={volumeValue} onChange={onVolumeChange} />
                <ToneDial
                  label="Treble"
                  gain={trebleGain}
                  onChange={(gain) => applyToneGain(TREBLE_BAND_INDEXES, gain)}
                />
              </div>
              <div className="mnp-eq-section">
                <div className="mnp-eq-header">
                  <span>Equalizer</span>
                  <label className="eq-enable">
                    <input
                      type="checkbox"
                      checked={eqSettings.enabled}
                      onChange={(e) => onEqEnabledChange(e.target.checked)}
                    />
                    On
                  </label>
                </div>
                <select
                  className="eq-preset-select mnp-eq-preset"
                  value=""
                  onChange={(e) => {
                    if (e.target.value) onEqPreset(e.target.value);
                  }}
                  aria-label="EQ preset"
                >
                  <option value="" disabled>
                    Presets
                  </option>
                  {EQ_PRESETS.map((preset) => (
                    <option key={preset.id} value={preset.id}>
                      {preset.label}
                    </option>
                  ))}
                </select>
                <div
                  className={`mnp-eq-bands ${eqSettings.enabled ? "" : "disabled"}`}
                >
                  {EQ_BAND_LABELS.map((label, index) => (
                    <div className="eq-band" key={label}>
                      <span className="eq-band-gain">
                        {(eqSettings.bands[index] ?? 0) > 0 ? "+" : ""}
                        {(eqSettings.bands[index] ?? 0).toFixed(0)}
                      </span>
                      <input
                        type="range"
                        min={-12}
                        max={12}
                        step={0.5}
                        value={eqSettings.bands[index] ?? 0}
                        onChange={(e) =>
                          onEqBandChange(index, Number(e.target.value))
                        }
                        aria-label={`${label} Hz`}
                      />
                      <span className="eq-band-label">{label}</span>
                    </div>
                  ))}
                </div>
                <button
                  className="btn-ghost btn-sm mnp-eq-reset"
                  onClick={onEqReset}
                  type="button"
                >
                  Reset EQ
                </button>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
