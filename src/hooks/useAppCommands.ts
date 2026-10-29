/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef } from "react";

/** Commands the macOS menu bar sends, and the shortcuts that run them. */
export type AppCommand =
  | "play_pause"
  | "next"
  | "previous"
  | "volume_up"
  | "volume_down"
  | "shuffle"
  | "repeat"
  | "search"
  | "queue"
  | "lyrics"
  | "mini_player"
  | "settings"
  | "open_files"
  | "add_folder"
  | "new_playlist"
  | "check_updates";

/**
 * Cmd on macOS, Ctrl elsewhere. On macOS the menu bar owns these keys and
 * sends `menu-command`, so the keydown only arrives on Windows and Linux.
 */
const SHORTCUTS: { key: string; shift?: boolean; command: AppCommand }[] = [
  { key: "ArrowRight", command: "next" },
  { key: "ArrowLeft", command: "previous" },
  { key: "ArrowUp", command: "volume_up" },
  { key: "ArrowDown", command: "volume_down" },
  { key: "s", command: "shuffle" },
  { key: "r", command: "repeat" },
  { key: "f", command: "search" },
  { key: "u", command: "queue" },
  { key: "l", command: "lyrics" },
  { key: "m", shift: true, command: "mini_player" },
  { key: ",", command: "settings" },
  { key: "o", command: "open_files" },
  { key: "o", shift: true, command: "add_folder" },
  { key: "n", command: "new_playlist" },
];

/** Arrow shortcuts leave a text field alone, so the caret still moves. */
const isTextField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.isContentEditable);

/** Desktop menu commands and keyboard shortcuts. */
export function useAppCommands(
  enabled: boolean,
  run: (command: AppCommand) => void,
) {
  const latest = useRef(run);
  latest.current = run;

  useEffect(() => {
    if (!enabled) return;
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
      const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
      const match = SHORTCUTS.find(
        (s) => s.key === key && !!s.shift === event.shiftKey,
      );
      if (!match) return;
      if (key.startsWith("Arrow") && isTextField(event.target)) return;
      event.preventDefault();
      latest.current(match.command);
    };
    window.addEventListener("keydown", onKey);

    let stop: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<AppCommand>("menu-command", ({ payload }) =>
          latest.current(payload),
        ),
      )
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});

    return () => {
      window.removeEventListener("keydown", onKey);
      cancelled = true;
      stop?.();
    };
  }, [enabled]);
}
