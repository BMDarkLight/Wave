import { useEffect, useRef, useState, type RefObject } from "react";
import type { LyricsLine } from "../utils/player";

/**
 * Renders timed lyrics, wiping through the active line word by word when the
 * source carries word timings.
 *
 * The wipe is driven by CSS, not by JavaScript. Playback position only arrives
 * every 500ms — far too coarse for words that last 200-400ms — so instead of
 * re-rendering at 60fps, each word in the active line is given a CSS animation
 * whose `animation-delay` is its offset from a captured anchor. A negative
 * delay starts an animation part-way through, so a word already half-sung
 * renders half-wiped immediately, and the compositor animates the rest for
 * free. Re-renders keep producing identical delay values while the anchor
 * holds, so React never touches the DOM and the animation is never restarted.
 */
export function LyricsLines({
  lines,
  activeIndex,
  position,
  isPlaying,
  onSeekToLine,
  activeLineRef,
  seekDisabled = false,
}: {
  lines: LyricsLine[];
  activeIndex: number;
  /** Latest known playback position, in seconds. */
  position: number;
  isPlaying: boolean;
  onSeekToLine: (time: number) => void;
  activeLineRef?: RefObject<HTMLButtonElement | null>;
  seekDisabled?: boolean;
}) {
  // Anchor: the position we believe playback was at, and when we believed it.
  // Word delays are measured from this, so it must only move when our belief
  // is actually wrong — otherwise every 500ms poll would restart the wipe.
  const [anchor, setAnchor] = useState({ position: 0, id: 0 });
  const anchorRef = useRef({ position: 0, at: 0 });

  useEffect(() => {
    const now = performance.now();
    const previous = anchorRef.current;
    const projected = previous.at
      ? previous.position + (now - previous.at) / 1000
      : Number.NEGATIVE_INFINITY;

    // Re-anchor on a seek, a track change, or a play/pause edge — anything
    // that makes the projection disagree with reality by more than the poll
    // interval can explain.
    if (!isPlaying || Math.abs(projected - position) > 0.6) {
      anchorRef.current = { position, at: now };
      setAnchor((prev) => ({ position, id: prev.id + 1 }));
    }
  }, [position, isPlaying]);

  return (
    <div className="lyrics-lines">
      {lines.map((line, index) => {
        const active = index === activeIndex;
        const karaoke = active && line.words.length > 0;
        return (
          <button
            key={`${line.time}-${index}`}
            ref={active ? activeLineRef : null}
            type="button"
            className={[
              "lyrics-line",
              active ? "active" : "",
              line.background ? "lyrics-line-bg" : "",
              karaoke ? "lyrics-line-karaoke" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            data-agent={line.agent ?? undefined}
            onClick={() => {
              if (seekDisabled) return;
              onSeekToLine(line.time);
            }}
            disabled={seekDisabled}
            title={seekDisabled ? undefined : "Jump to this line"}
          >
            {karaoke
              ? line.words.map((word, wordIndex) => (
                  <span
                    key={`${word.time}-${wordIndex}`}
                    className="lyrics-word"
                    style={{
                      animationDelay: `${word.time - anchor.position}s`,
                      animationDuration: `${Math.max(word.end - word.time, 0.05)}s`,
                      animationPlayState: isPlaying ? "running" : "paused",
                    }}
                  >
                    {word.text}
                  </span>
                ))
              : line.text || " "}
          </button>
        );
      })}
    </div>
  );
}
