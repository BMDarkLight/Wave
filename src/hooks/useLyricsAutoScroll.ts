import { useCallback, useEffect, useRef, type RefObject } from "react";

const LYRICS_SCROLL_PAUSE_MS = 3000;

/** Auto-scrolls to the active lyric line unless the user scrolled recently. */
export function useLyricsAutoScroll(
  activeIndex: number,
  enabled: boolean,
  activeLineRef: RefObject<HTMLElement | null>,
) {
  const pausedUntilRef = useRef(0);
  const isAutoScrollingRef = useRef(false);

  const pauseAutoScroll = useCallback(() => {
    pausedUntilRef.current = Date.now() + LYRICS_SCROLL_PAUSE_MS;
  }, []);

  const onLyricsScroll = useCallback(() => {
    if (isAutoScrollingRef.current) return;
    pauseAutoScroll();
  }, [pauseAutoScroll]);

  useEffect(() => {
    if (!enabled || activeIndex < 0 || !activeLineRef.current) return;
    if (Date.now() < pausedUntilRef.current) return;

    isAutoScrollingRef.current = true;
    activeLineRef.current.scrollIntoView({
      block: "center",
      behavior: "smooth",
    });

    const timer = window.setTimeout(() => {
      isAutoScrollingRef.current = false;
    }, 800);

    return () => {
      window.clearTimeout(timer);
      isAutoScrollingRef.current = false;
    };
  }, [activeIndex, enabled, activeLineRef]);

  return {
    onLyricsScroll,
    onLyricsTouchStart: pauseAutoScroll,
    onLyricsWheel: pauseAutoScroll,
  };
}
