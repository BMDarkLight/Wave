/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import type { Theme } from "./player";

/**
 * The Appearance switches in Settings work as classes on `<html>`, so the
 * stylesheet can turn the blur and the motion down everywhere at once. Code
 * that animates outside CSS (smooth scrolling, canvas drawing) asks
 * `lessMotion()` instead. The theme works the same way, as `data-theme`.
 */

const NO_GLASS = "no-glass";
const NO_MOTION = "no-motion";
const REDUCE_MOTION = "reduce-motion";
/** Read by the script in index.html, which paints the theme before CSS. */
const THEME_KEY = "wave-theme";

/**
 * Show `theme` and remember it, so the next launch starts in it instead of
 * flashing the other one while the settings load.
 */
export const applyTheme = (theme: Theme) => {
  document.documentElement.dataset.theme = theme;
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", theme === "light" ? "#ffffff" : "#000000");
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    // Storage can be off; the next launch just starts from the system.
  }
};

export const applyGlassEffects = (enabled: boolean) => {
  document.documentElement.classList.toggle(NO_GLASS, !enabled);
};

/** Reduce motion only means something while animations are on. */
export const applyMotion = (animations: boolean, reduce: boolean) => {
  const root = document.documentElement.classList;
  root.toggle(NO_MOTION, !animations);
  root.toggle(REDUCE_MOTION, animations && reduce);
};

/** True when animations are off or reduced. */
export const lessMotion = (): boolean => {
  const root = document.documentElement.classList;
  return root.contains(NO_MOTION) || root.contains(REDUCE_MOTION);
};
