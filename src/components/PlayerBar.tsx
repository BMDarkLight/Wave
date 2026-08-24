import type { RefObject } from "react";
import {
  BiListUl,
  BiMusic,
  BiPause,
  BiPlay,
  BiRepeat,
  BiShuffle,
  BiSkipNext,
  BiSkipPrevious,
  BiStop,
  BiVolumeFull,
  BiVolumeLow,
  BiVolumeMute,
} from "react-icons/bi";
import Artwork from "./Artwork";
import { formatTime } from "../utils/format";
import { getTrackTitle } from "../utils/track";
import type { EqSettings, PlaybackMode, PlaybackState, Track } from "../utils/player";

export default function PlayerBar({
  currentTrack,
  playbackState,
  playbackMode,
  displayPosition,
  displayDuration,
  canSkip,
  showQueue,
  showEqPanel,
  eqSettings,
  volumeValue,
  lyricsPanelTrack,
  coverLetters,
  isMobileLayout,
  mobilePlayerOpenRef,
  mobilePlayerClosingRef,
  mobilePlayerClosing,
  volumeIconRef,
  onOpenMobilePlayer,
  onOpenNowPlaying,
  onOpenArtist,
  onOpenAlbum,
  onToggleShuffle,
  onPrevious,
  onStop,
  onPlayPause,
  onNext,
  onCycleRepeat,
  onSeekChange,
  onSeekCommit,
  onToggleLyrics,
  onToggleQueue,
  onToggleEqPanel,
  onVolumeChange,
  onToggleDevice,
  onRefreshOutputDevices,
}: {
  currentTrack: Track | null;
  playbackState: PlaybackState;
  playbackMode: PlaybackMode;
  displayPosition: number;
  displayDuration: number;
  canSkip: boolean;
  showQueue: boolean;
  showEqPanel: boolean;
  eqSettings: EqSettings;
  volumeValue: number;
  lyricsPanelTrack: Track | null;
  coverLetters: string;
  isMobileLayout: () => boolean;
  mobilePlayerOpenRef: RefObject<boolean>;
  mobilePlayerClosingRef: RefObject<boolean>;
  mobilePlayerClosing: boolean;
  volumeIconRef: RefObject<HTMLButtonElement | null>;
  onOpenMobilePlayer: () => void;
  onOpenNowPlaying: () => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  onToggleShuffle: () => void;
  onPrevious: () => void;
  onStop: () => void;
  onPlayPause: () => void;
  onNext: () => void;
  onCycleRepeat: () => void;
  onSeekChange: (value: number) => void;
  onSeekCommit: (value: number) => void;
  onToggleLyrics: () => void;
  onToggleQueue: () => void;
  onToggleEqPanel: () => void;
  onVolumeChange: (value: number) => void;
  onToggleDevice: () => void;
  onRefreshOutputDevices: () => void;
}) {
  return (
    <footer
      className={`player-bar${currentTrack && (!mobilePlayerOpenRef.current || mobilePlayerClosing) ? " player-bar-tappable" : ""}`}
      onClick={(event) => {
        // Tapping empty space in the mini player (mobile only) opens the
        // fullscreen Now Playing page. Clicks on transport/seek controls
        // are left alone; the title/art block opens NP on its own.
        if (!isMobileLayout()) return;
        // Use refs so a drag-dismiss (which clears openRef synchronously)
        // doesn't leave the first tap blocked on stale React state.
        if (mobilePlayerOpenRef.current && !mobilePlayerClosingRef.current) {
          return;
        }
        const target = event.target as HTMLElement;
        if (
          target.closest(
            ".player-controls, .seek-row, .player-right, input, select, a",
          )
        ) {
          return;
        }
        if (!currentTrack) return;
        onOpenMobilePlayer();
      }}
    >
      <div className="player-left">
        <button
          className="album-art-btn"
          onClick={onOpenNowPlaying}
          disabled={!currentTrack}
          type="button"
          title={currentTrack ? "Open now playing" : undefined}
        >
          <Artwork
            track={currentTrack}
            fallback={coverLetters}
            className="album-art"
          />
        </button>
        <div
          className="now-playing-info"
          onClick={() => {
            // Mobile: the whole info block opens Now Playing (large hit
            // target). Desktop keeps per-field buttons below.
            if (!isMobileLayout() || !currentTrack) return;
            if (mobilePlayerOpenRef.current && !mobilePlayerClosingRef.current) {
              return;
            }
            onOpenMobilePlayer();
          }}
        >
          <button
            type="button"
            className="now-playing-name"
            onClick={onOpenNowPlaying}
            disabled={!currentTrack}
            title={currentTrack ? "Open now playing" : undefined}
          >
            {getTrackTitle(currentTrack, playbackState.current_path)}
          </button>
          <button
            className="now-playing-artist"
            onClick={() => {
              if (!currentTrack?.artist) return;
              // Artist page is reached from Now Playing on mobile — tapping
              // the bar artist should open NP, not navigate away.
              if (isMobileLayout()) {
                onOpenMobilePlayer();
                return;
              }
              onOpenArtist(currentTrack.artist);
            }}
            type="button"
            disabled={!currentTrack?.artist}
          >
            {currentTrack?.artist ??
              (playbackState.current_path ? "Local file" : "No track selected")}
          </button>
          <button
            className="now-playing-path"
            onClick={() => {
              if (!currentTrack?.album) return;
              onOpenAlbum(currentTrack.album, currentTrack.album_artist || currentTrack.artist);
            }}
            type="button"
            disabled={!currentTrack?.album}
          >
            {currentTrack?.album ??
              playbackState.current_path ??
              "Add music to your playlist"}
          </button>
        </div>
      </div>

      <div className="player-controls">
        <button
          className={`control-btn shuffle-btn ${playbackMode.shuffle ? "active" : ""}`}
          onClick={onToggleShuffle}
          type="button"
          title={playbackMode.shuffle ? "Disable shuffle" : "Enable shuffle"}
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
          className="control-btn desktop-only-control"
          onClick={onStop}
          disabled={!playbackState.current_path}
          type="button"
          title="Stop"
        >
          <BiStop />
        </button>
        <button
          className="control-btn play-pause-btn"
          onClick={onPlayPause}
          type="button"
          title="Play/Pause"
        >
          {playbackState.is_playing ? <BiPause /> : <BiPlay />}
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
          title={
            playbackMode.repeat === "off"
              ? "Repeat off"
              : playbackMode.repeat === "all"
                ? "Repeat all"
                : "Repeat one"
          }
        >
          <BiRepeat />
        </button>
      </div>

      <div className="seek-row">
        <span>{formatTime(displayPosition)}</span>
        <input
          className="range-slider"
          type="range"
          min="0"
          max={Math.max(displayDuration, 1)}
          step="1"
          value={displayPosition}
          disabled={!playbackState.current_path}
          onPointerDown={() => document.body.classList.add("is-seeking")}
          onPointerCancel={() => document.body.classList.remove("is-seeking")}
          onChange={(event) => onSeekChange(Number(event.target.value))}
          onPointerUp={(event) => onSeekCommit(Number(event.currentTarget.value))}
        />
        <span>{formatTime(displayDuration)}</span>
      </div>

      <div className="player-right">
        <div className="player-right-row">
          {currentTrack?.lyrics && (
            <button
              className={`control-btn lyrics-btn ${lyricsPanelTrack ? "active" : ""}`}
              onClick={onToggleLyrics}
              type="button"
              title="Toggle lyrics"
            >
              <BiMusic />
            </button>
          )}
          <button
            className={`control-btn queue-toggle desktop-queue-btn ${showQueue ? "active" : ""}`}
            onClick={onToggleQueue}
            type="button"
            title="Toggle queue"
          >
            <BiListUl />
          </button>
          <span
            className={`status-dot ${playbackState.is_playing ? "playing" : playbackState.is_paused ? "paused" : ""}`}
          />
          <button
            ref={volumeIconRef}
            className={`volume-icon desktop-only-control ${showEqPanel ? "active" : ""} ${eqSettings.enabled ? "eq-on" : ""}`}
            onClick={onToggleEqPanel}
            type="button"
            title="Equalizer"
            aria-label="Open equalizer"
          >
            {volumeValue === 0 ? (
              <BiVolumeMute />
            ) : volumeValue < 0.5 ? (
              <BiVolumeLow />
            ) : (
              <BiVolumeFull />
            )}
          </button>
          <input
            className="range-slider volume"
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={volumeValue}
            onChange={(event) => onVolumeChange(Number(event.target.value))}
          />
          <span className="volume-percent">
            {Math.round(volumeValue * 100)}%
          </span>
        </div>
        <div className="device-selector">
          <button
            className="output-device-name"
            onClick={() => {
              onRefreshOutputDevices();
              onToggleDevice();
            }}
            title="Click to change audio output device"
            type="button"
          >
            {playbackState.output_device_name || "No device"}
          </button>
        </div>
      </div>
    </footer>
  );
}
