/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

/**
 * The Appearance switches in Settings work as classes on `<html>`, so the
 * stylesheet can turn the blur and the motion down everywhere at once. Code
 * that animates outside CSS (smooth scrolling, canvas drawing) asks
 * `lessMotion()` instead.
 */

const NO_GLASS = "no-glass";
const NO_MOTION = "no-motion";
const REDUCE_MOTION = "reduce-motion";

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
