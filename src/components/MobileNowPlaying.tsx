// Mobile-only fullscreen "Now Playing" page. Replaces the desktop lyrics
// sidebar on narrow/responsive layouts: big cover art, transport controls,
// a lyrics view toggle, and a bottom-sheet menu with the volume dial + EQ.
import { useEffect, useMemo, useRef, useState } from "react";
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
  BiMusic,
  BiListUl,
  BiSliderAlt,
  BiX,
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

/** Circular drag-to-set volume knob (270° sweep, gap at the bottom). */
function VolumeDial({
  value,
  onChange,
}: {
  value: number;
  onChange: (value: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const size = 128;
  const r = 46;
  const cx = size / 2;
  const cy = size / 2;

  const updateFromPointer = (clientX: number, clientY: number) => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const dx = clientX - centerX;
    const dy = clientY - centerY;
    const deg = Math.atan2(dy, dx) * (180 / Math.PI); // 0=right,90=down,-90=up
    let a = deg + 90; // 0=up, 90=right, 180=down, 270=left (clockwise)
    a = ((a % 360) + 360) % 360;
    let shifted = a - 225; // 0 at bottom-left start, increases clockwise
    if (shifted < 0) shifted += 360;
    let next: number;
    if (shifted <= 270) {
      next = shifted / 270;
    } else {
      // In the bottom "gap" — snap to whichever end is nearer.
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
  const valuePoint = pointFor(angleFor(value));
  const valueSweepDeg = Math.max(0, Math.min(1, value)) * 270;

  const VolumeIcon =
    value === 0 ? BiVolumeMute : value < 0.5 ? BiVolumeLow : BiVolumeFull;

  return (
    <div
      ref={ref}
      className={`mnp-dial ${dragging ? "dragging" : ""}`}
      role="slider"
      tabIndex={0}
      aria-label="Volume"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(value * 100)}
      onPointerDown={(e) => {
        e.preventDefault();
        setDragging(true);
        updateFromPointer(e.clientX, e.clientY);
      }}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp" || e.key === "ArrowRight") {
          e.preventDefault();
          onChange(Math.min(1, value + 0.05));
        } else if (e.key === "ArrowDown" || e.key === "ArrowLeft") {
          e.preventDefault();
          onChange(Math.max(0, value - 0.05));
        }
      }}
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <path
          d={`M ${startPoint.x} ${startPoint.y} A ${r} ${r} 0 1 1 ${endPoint.x} ${endPoint.y}`}
          fill="none"
          stroke="rgba(255,255,255,0.15)"
          strokeWidth={10}
          strokeLinecap="round"
        />
        {value > 0 && (
          <path
            d={`M ${startPoint.x} ${startPoint.y} A ${r} ${r} 0 ${valueSweepDeg > 180 ? 1 : 0} 1 ${valuePoint.x} ${valuePoint.y}`}
            fill="none"
            stroke="#fff"
            strokeWidth={10}
            strokeLinecap="round"
          />
        )}
        <circle cx={valuePoint.x} cy={valuePoint.y} r={7} fill="#fff" />
      </svg>
      <div className="mnp-dial-center">
        <VolumeIcon />
        <span>{Math.round(value * 100)}%</span>
      </div>
    </div>
  );
}

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
  onClose: () => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onOpenQueue: () => void;
  volumeValue: number;
  onVolumeChange: (value: number) => void;
  eqSettings: EqSettings;
  onEqEnabledChange: (enabled: boolean) => void;
  onEqBandChange: (index: number, gain: number) => void;
  onEqPreset: (id: string) => void;
  onEqReset: () => void;
  showLyrics: boolean;
  onShowLyricsChange: (open: boolean) => void;
  menuOpen: boolean;
  onMenuOpenChange: (open: boolean) => void;
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
  onClose,
  onOpenArtist,
  onOpenAlbum,
  onOpenQueue,
  volumeValue,
  onVolumeChange,
  eqSettings,
  onEqEnabledChange,
  onEqBandChange,
  onEqPreset,
  onEqReset,
  showLyrics,
  onShowLyricsChange,
  menuOpen,
  onMenuOpenChange,
}: MobileNowPlayingProps) {
  const [fullCover, setFullCover] = useState<string | null>(null);
  const [lyricsText, setLyricsText] = useState<string | null>(
    track.lyrics ?? null,
  );
  const [lyricsSource, setLyricsSource] = useState<string | null>(
    track.lyrics_source ?? null,
  );
  const activeLineRef = useRef<HTMLButtonElement>(null);

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
    if (showLyrics && activeLineRef.current) {
      activeLineRef.current.scrollIntoView({
        block: "center",
        behavior: "smooth",
      });
    }
  }, [activeLyricIndex, showLyrics]);

  const title = getTrackTitle(track);
  const coverLetters = title.slice(0, 2).toUpperCase();

  return (
    <div className="mobile-now-playing">
      <div className="mnp-header">
        <button
          className="mnp-icon-btn"
          onClick={onClose}
          type="button"
          aria-label="Minimize player"
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
        {showLyrics ? (
          <div className="mnp-lyrics-scroll">
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
        ) : (
          <div className="mnp-cover-wrap">
            <Artwork
              track={track}
              overrideSrc={fullCover}
              fallback={coverLetters}
              className="mnp-cover"
            />
          </div>
        )}
      </div>

      <div className="mnp-meta">
        <div className="mnp-meta-text">
          <div className="mnp-title" title={title}>
            {title}
          </div>
          <button
            className="mnp-artist"
            onClick={() => track.artist && onOpenArtist(track.artist)}
            type="button"
            disabled={!track.artist}
          >
            {track.artist || "Unknown artist"}
          </button>
        </div>
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
          className={`mnp-action-btn ${showLyrics ? "active" : ""}`}
          onClick={() => onShowLyricsChange(!showLyrics)}
          type="button"
          title="Lyrics"
          aria-label="Toggle lyrics"
        >
          <BiMusic />
        </button>
        <button
          className="mnp-action-btn"
          onClick={onOpenQueue}
          type="button"
          title="Queue"
          aria-label="Open queue"
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

      {menuOpen && (
        <>
          <button
            className="mnp-sheet-backdrop"
            onClick={() => onMenuOpenChange(false)}
            type="button"
            aria-label="Close menu"
          />
          <div className="mnp-sheet" role="dialog" aria-label="Volume and equalizer">
            <div className="mnp-sheet-handle" />
            <div className="mnp-sheet-header">
              <h3>Playback</h3>
              <button
                className="mnp-icon-btn"
                onClick={() => onMenuOpenChange(false)}
                type="button"
                aria-label="Close"
              >
                <BiX />
              </button>
            </div>
            <div className="mnp-sheet-scroll">
              <div className="mnp-volume-section">
                <VolumeDial value={volumeValue} onChange={onVolumeChange} />
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
