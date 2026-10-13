/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import {
  getThemeMode,
  setSystemBarTheme,
  setThemeMode,
  watchSystemTheme,
  type Theme,
  type ThemeMode,
} from "../utils/player";
import { applyTheme } from "../utils/appearance";
import { formatInvokeError } from "../utils/errors";

/**
 * The theme choice from Settings, and the theme on screen. "System" follows
 * the operating system's light or dark setting as it changes.
 */
export function useTheme(setError: (message: string | null) => void) {
  // Unknown until the saved choice arrives. Until then the page keeps the
  // theme index.html painted from the last launch.
  const [themeMode, setThemeModeState] = useState<ThemeMode | null>(null);
  const [systemTheme, setSystemTheme] = useState<Theme | null>(null);

  useEffect(() => {
    getThemeMode()
      .then(setThemeModeState)
      .catch(() => setThemeModeState("system"));
  }, []);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    watchSystemTheme(setSystemTheme)
      .then((unwatch) => {
        if (cancelled) unwatch();
        else stop = unwatch;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const theme: Theme | null =
    themeMode === null
      ? null
      : themeMode === "system"
        ? systemTheme
        : themeMode;

  useEffect(() => {
    if (!theme) return;
    applyTheme(theme);
    setSystemBarTheme(theme).catch(() => {});
  }, [theme]);

  const handleThemeModeChange = async (mode: ThemeMode) => {
    const previous = themeMode;
    setThemeModeState(mode);
    try {
      await setThemeMode(mode);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to update the theme"));
      setThemeModeState(previous);
    }
  };

  return {
    themeMode: themeMode ?? "system",
    handleThemeModeChange,
  };
}
