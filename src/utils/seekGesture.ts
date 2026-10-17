/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// While a seek bar is held, playback polls leave the position alone so the
// thumb stays under the finger. The flag lives on <body> because every seek
// bar and the poll in usePlaybackController share it.
//
// It used to be cleared only by the slider's own pointerup or pointercancel.
// A release the slider never saw (the finger slid onto the swipe-to-close
// sheet, or the slider unmounted mid-drag) left it set for good, and the
// position stopped updating until the next seek. Ending on any release in
// the window covers those.

const SEEKING_CLASS = "is-seeking";

let releaseListening = false;

const onRelease = () => endSeekGesture();

export const isSeeking = (): boolean =>
  document.body.classList.contains(SEEKING_CLASS);

export function beginSeekGesture(): void {
  document.body.classList.add(SEEKING_CLASS);
  if (releaseListening) return;
  releaseListening = true;
  // Bubble phase on the window runs after React's handlers, so the slider's
  // own pointerup commits the seek before the flag goes.
  window.addEventListener("pointerup", onRelease);
  window.addEventListener("pointercancel", onRelease);
  window.addEventListener("blur", onRelease);
}

export function endSeekGesture(): void {
  document.body.classList.remove(SEEKING_CLASS);
  if (!releaseListening) return;
  releaseListening = false;
  window.removeEventListener("pointerup", onRelease);
  window.removeEventListener("pointercancel", onRelease);
  window.removeEventListener("blur", onRelease);
}
