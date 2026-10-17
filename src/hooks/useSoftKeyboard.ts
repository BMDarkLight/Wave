/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect } from "react";

/** How much the page has to shrink before it counts as the keyboard. */
const KEYBOARD_MIN_HEIGHT = 120;

const TEXT_INPUT_TYPES = new Set([
  "",
  "text",
  "search",
  "email",
  "url",
  "tel",
  "password",
  "number",
]);

const takesTyping = (element: Element | null): boolean => {
  if (!(element instanceof HTMLElement)) return false;
  if (element.isContentEditable) return true;
  if (element instanceof HTMLTextAreaElement) return !element.readOnly;
  if (element instanceof HTMLInputElement) {
    return (
      TEXT_INPUT_TYPES.has(element.type.toLowerCase()) && !element.readOnly
    );
  }
  return false;
};

/**
 * Sets `keyboard-open` on <html> while the on-screen keyboard is up.
 *
 * On Android the web view is shrunk to the space above the keyboard (see
 * KeyboardInsets.java), so everything pinned to the bottom of the page, the
 * tabs and the mini player, would ride up on top of it and cover what is
 * being typed into. The page has no keyboard event, so it counts as open
 * while a text field has focus and the page is clearly shorter than it has
 * been at this width. Desktop windows do not shrink on focus, so the class
 * never appears there.
 */
export function useSoftKeyboard(): void {
  useEffect(() => {
    const root = document.documentElement;
    let width = window.innerWidth;
    let tallest = window.innerHeight;
    let frame = 0;

    const update = () => {
      frame = 0;
      const height = window.innerHeight;
      // A rotation or a resized window starts a new comparison.
      if (window.innerWidth !== width) {
        width = window.innerWidth;
        tallest = height;
      }
      tallest = Math.max(tallest, height);
      const open =
        takesTyping(document.activeElement) &&
        tallest - height >= KEYBOARD_MIN_HEIGHT;
      root.classList.toggle("keyboard-open", open);
    };

    // focusout fires before focus lands anywhere new, so look a frame later.
    const schedule = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };

    window.addEventListener("resize", schedule);
    document.addEventListener("focusin", schedule);
    document.addEventListener("focusout", schedule);
    update();
    return () => {
      if (frame !== 0) cancelAnimationFrame(frame);
      window.removeEventListener("resize", schedule);
      document.removeEventListener("focusin", schedule);
      document.removeEventListener("focusout", schedule);
      root.classList.remove("keyboard-open");
    };
  }, []);
}
