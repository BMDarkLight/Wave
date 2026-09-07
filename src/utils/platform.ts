/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { safeInvokeHostOs } from "./player";

let cachedHostOs: string | null = null;

/** Host OS from Rust (`android`, `ios`, `macos`, `linux`, `windows`, …). */
export async function getHostOs(): Promise<string> {
  if (cachedHostOs) return cachedHostOs;
  try {
    cachedHostOs = await safeInvokeHostOs();
  } catch {
    cachedHostOs = "unknown";
  }
  return cachedHostOs;
}

export async function isAndroid(): Promise<boolean> {
  return (await getHostOs()) === "android";
}

export async function isMobileHost(): Promise<boolean> {
  const os = await getHostOs();
  return os === "android" || os === "ios";
}

/** Narrow layout breakpoint used by Wave's mobile CSS. */
export function isMobileLayout(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(max-width: 900px)").matches;
}
