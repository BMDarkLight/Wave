/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// Things that open and close on their own state, such as a menu, a card or
// a search field, sign up here while they are open so the hardware back
// button closes them before it leaves the page.

import { useEffect, useRef, useSyncExternalStore } from "react";

/**
 * "popup" is anything floating over the page (menus, cards, dialogs) and is
 * closed first. "page" belongs to the page itself, like its search field,
 * and is closed just before back leaves the page.
 */
export type BackLayerLevel = "popup" | "page";

type Layer = { level: BackLayerLevel; close: () => void };

const layers: Layer[] = [];
const listeners = new Set<() => void>();
let version = 0;

const changed = () => {
  version += 1;
  for (const listener of listeners) listener();
};

const remove = (layer: Layer) => {
  const index = layers.indexOf(layer);
  if (index === -1) return;
  layers.splice(index, 1);
  changed();
};

/** How many layers of `level` are open. */
export const openBackLayers = (level: BackLayerLevel): number =>
  layers.filter((layer) => layer.level === level).length;

/**
 * Close the most recently opened layer of `level`. It leaves the list at
 * once, so a second quick back press goes to the next one down.
 */
export const closeTopBackLayer = (level: BackLayerLevel): boolean => {
  for (let i = layers.length - 1; i >= 0; i--) {
    const layer = layers[i];
    if (layer.level !== level) continue;
    remove(layer);
    layer.close();
    return true;
  }
  return false;
};

/** Changes whenever a layer opens or closes, for effects to depend on. */
export const useBackLayersVersion = (): number =>
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => version,
  );

/** Keep a layer signed up while `open` is true. */
export function useBackLayer(
  open: boolean,
  close: () => void,
  level: BackLayerLevel = "popup",
) {
  const closeRef = useRef(close);
  closeRef.current = close;

  useEffect(() => {
    if (!open) return;
    const layer: Layer = { level, close: () => closeRef.current() };
    layers.push(layer);
    changed();
    return () => remove(layer);
  }, [open, level]);
}
