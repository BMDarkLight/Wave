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
 * Thin bindings for `tauri-plugin-media-session`.
 *
 * The published npm package is not always available, so we invoke the plugin
 * commands directly via `@tauri-apps/api`.
 */

import type { PluginListener } from "@tauri-apps/api/core";

export type MediaAction =
  | "play"
  | "pause"
  | "stop"
  | "next"
  | "previous"
  | "seek"
  | "shuffle"
  | "repeat";

export interface MediaActionEvent {
  action: MediaAction;
  seekPosition?: number;
}

/** Listen for notification / lockscreen / Bluetooth media button actions. */
export async function onMediaSessionAction(
  handler: (event: MediaActionEvent) => void,
): Promise<PluginListener | null> {
  try {
    const { addPluginListener } = await import("@tauri-apps/api/core");
    return await addPluginListener<MediaActionEvent>(
      "media-session",
      "media_action",
      handler,
    );
  } catch (error) {
    console.warn("media-session plugin listener unavailable:", error);
    return null;
  }
}
