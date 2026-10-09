/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, type DependencyList } from "react";

/** Scroll distance (px) over which the fade comes all the way in. */
const FADE_IN_DISTANCE = 48;

/** Full-screen layers that sit above the fade; their scrolling is theirs. */
const OVERLAYS = ".mobile-now-playing, .mobile-settings-page, .mnp-sheet";

/**
 * Drives the fade under the status bar on the narrow layout: clear while the
 * page is at its top, coming in as it scrolls down. Pages scroll in different
 * elements, so this listens for any vertical scroll and reads that element.
 * `resetDeps` should change whenever a different page is shown, since a new
 * page starts at its top without firing a scroll event.
 */
export function useStatusFade(resetDeps: DependencyList) {
  const fadeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const lastTop = new WeakMap<Element, number>();
    const onScroll = (event: Event) => {
      const target = event.target;
      if (!(target instanceof Element) || target.closest(OVERLAYS)) return;
      // Rows that scroll sideways fire scroll events too; only a change in
      // vertical position says anything about the page. Everything starts
      // at its top, so an element not seen before counts as 0.
      const top = target.scrollTop;
      if ((lastTop.get(target) ?? 0) === top) return;
      lastTop.set(target, top);
      if (fadeRef.current) {
        fadeRef.current.style.opacity = String(
          Math.min(1, Math.max(0, top / FADE_IN_DISTANCE)),
        );
      }
    };
    document.addEventListener("scroll", onScroll, {
      capture: true,
      passive: true,
    });
    return () => document.removeEventListener("scroll", onScroll, true);
  }, []);

  useEffect(() => {
    if (fadeRef.current) fadeRef.current.style.opacity = "0";
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, resetDeps);

  return fadeRef;
}
