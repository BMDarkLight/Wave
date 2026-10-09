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
 * Share of the height the loudest bar takes at rest. The rest is headroom
 * for the live level to lift bars into.
 */
export const REST_HEIGHT = 0.7;

/** Bars either side of the playhead that react to the live level. */
export const LIVE_REACH = 10;

/**
 * Fold the stored waveform (0-255 per value) into `count` bars, each keeping
 * the loudest value it covers.
 */
export const barHeights = (bins: number[], count: number): number[] => {
  const bars = new Array<number>(count).fill(0);
  if (bins.length === 0 || count <= 0) return bars;
  for (let bar = 0; bar < count; bar++) {
    const start = Math.floor((bar * bins.length) / count);
    const end = Math.max(
      start + 1,
      Math.floor(((bar + 1) * bins.length) / count),
    );
    let loudest = 0;
    for (let i = start; i < end && i < bins.length; i++) {
      loudest = Math.max(loudest, bins[i]);
    }
    bars[bar] = loudest;
  }
  return bars;
};

/**
 * How much a bar `distance` bars from the playhead grows with the live
 * level: up to 1.4x right at the playhead, fading to 1x at
 * {@link LIVE_REACH}.
 */
export const liveLift = (level: number, distance: number): number => {
  if (distance >= LIVE_REACH) return 1;
  // Music RMS rarely passes 0.3, so stretch it to fill the range.
  const loudness = Math.min(1, level * 3);
  return 1 + 0.4 * loudness * (1 - distance / LIVE_REACH);
};

/** Ease the drawn level toward the measured one: quick up, slow down. */
export const smoothLevel = (shown: number, measured: number): number =>
  shown + (measured - shown) * (measured > shown ? 0.5 : 0.12);

/** How often a new live reading enters the played side, in ms. */
export const LIVE_STEP_MS = 50;

/** Live readings kept for the played side, more than any bar count needs. */
export const LIVE_HISTORY = 1200;

/**
 * Height of a live bar from a reading, 0 to 1. Music RMS rarely passes 0.3,
 * so it is stretched, and the curve lifts quiet passages so they still move.
 */
export const liveHeight = (level: number): number =>
  Math.min(1, level * 3) ** 0.7;

/** Ease the played side toward live while playing and back when paused. */
export const stepLiveMix = (mix: number, playing: boolean): number => {
  const next = mix + ((playing ? 1 : 0) - mix) * 0.1;
  return Math.abs(next - (playing ? 1 : 0)) < 0.002 ? (playing ? 1 : 0) : next;
};
