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
 * stylesheet can turn the blur and the motion off everywhere at once. Code
 * that animates outside CSS (smooth scrolling, canvas drawing) asks
 * `animationsOff()` instead.
 */

const NO_GLASS = "no-glass";
const NO_MOTION = "no-motion";

export const applyGlassEffects = (enabled: boolean) => {
  document.documentElement.classList.toggle(NO_GLASS, !enabled);
};

export const applyAnimations = (enabled: boolean) => {
  document.documentElement.classList.toggle(NO_MOTION, !enabled);
};

export const animationsOff = (): boolean =>
  document.documentElement.classList.contains(NO_MOTION);
