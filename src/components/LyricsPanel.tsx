import type { RefObject } from "react";
import { BiX } from "react-icons/bi";
import Artwork from "./Artwork";
import { getTrackTitle } from "../utils/track";
import type { LyricLine } from "../utils/lyrics";
import type { Track } from "../utils/player";
import type { useLyricsAutoScroll } from "../hooks/useLyricsAutoScroll";

export default function LyricsPanel({
  track,
  fullCover,
  onClose,
  onOpenArtist,
  onOpenAlbum,
  timedLyrics,
  activeLyricIndex,
  activeLyricLineRef,
  isCurrentTrack,
  onSeek,
  scrollHandlers,
}: {
  track: Track;
  fullCover: string | null;
  onClose: () => void;
  onOpenArtist: (artist: string) => void;
  onOpenAlbum: (album: string, albumArtist: string | null) => void;
  timedLyrics: LyricLine[] | null;
  activeLyricIndex: number;
  activeLyricLineRef: RefObject<HTMLButtonElement | null>;
  isCurrentTrack: boolean;
  onSeek: (time: number) => void;
  scrollHandlers: ReturnType<typeof useLyricsAutoScroll>;
}) {
  return (
    <div className="right-panel-content lyrics-panel">
      <button
        className="right-panel-close lyrics-close-float"
        onClick={onClose}
        type="button"
        title="Close"
      >
        <BiX />
      </button>
      <div
        className="lyrics-panel-scroll"
        onScroll={scrollHandlers.onLyricsScroll}
        onTouchStart={scrollHandlers.onLyricsTouchStart}
        onWheel={scrollHandlers.onLyricsWheel}
      >
        <div className="lyrics-panel-cover">
          <Artwork
            track={track}
            overrideSrc={fullCover}
            fallback={getTrackTitle(track).slice(0, 2).toUpperCase()}
            className="lyrics-cover"
          />
        </div>
        <div className="lyrics-panel-header">
          <div className="right-panel-header">
            <h2>{getTrackTitle(track)}</h2>
          </div>
          {track.artist && (
            <p className="lyrics-artist">
              by{" "}
              <button
                className="lyrics-link"
                onClick={() => {
                  onOpenArtist(track.artist);
                  onClose();
                }}
                type="button"
              >
                {track.artist}
              </button>
            </p>
          )}
          {track.album && (
            <p className="lyrics-album">
              From{" "}
              <button
                className="lyrics-link"
                onClick={() => {
                  onOpenAlbum(track.album, track.album_artist || track.artist);
                  onClose();
                }}
                type="button"
              >
                {track.album}
              </button>
            </p>
          )}
        </div>
        <div className="lyrics-panel-body">
          {timedLyrics ? (
            <div className="lyrics-lines">
              {timedLyrics.map((line, index) => (
                <button
                  key={`${line.time}-${index}`}
                  ref={index === activeLyricIndex ? activeLyricLineRef : null}
                  type="button"
                  className={`lyrics-line ${index === activeLyricIndex ? "active" : ""}`}
                  onClick={() => {
                    if (!isCurrentTrack) return;
                    onSeek(line.time);
                  }}
                  disabled={!isCurrentTrack}
                  title={isCurrentTrack ? "Jump to this line" : undefined}
                >
                  {line.text || " "}
                </button>
              ))}
            </div>
          ) : track.lyrics ? (
            <pre>{track.lyrics}</pre>
          ) : (
            <p className="lyrics-empty">No lyrics available</p>
          )}
          {track.lyrics && (
            <p className="lyrics-source">
              {track.lyrics_source === "lrclib"
                ? "Lyrics provided by LRCLIB"
                : "Lyrics pulled from the file"}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
