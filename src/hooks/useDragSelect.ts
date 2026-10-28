/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState, type PointerEvent } from "react";

/** How long a finger rests on a row before sliding selects instead of
 * scrolling. A mouse can slide straight away. */
const HOLD_MS = 350;
/** How far the pointer may drift and still count as resting. */
const SLOP_PX = 10;
/** How close to the list's top or bottom edge the pointer scrolls it. */
const EDGE_PX = 56;

type Gesture = {
  list: string;
  anchor: number;
  pointerId: number;
  mouse: boolean;
  startX: number;
  startY: number;
  x: number;
  y: number;
  timer: number;
  scroll: HTMLElement | null;
  sliding: boolean;
  frame: number;
  last: number;
  /** Takes off the window listeners this gesture added. */
  detach: () => void;
};

/**
 * Press a row and slide up or down to set every row passed over at once.
 * A finger holds first, so a plain swipe still scrolls; a mouse slides as
 * soon as it moves. Rows carry `data-drag-list` and `data-drag-index` (from
 * `rowProps`) so the pointer can be traced to a row as the list scrolls
 * under it.
 */
export function useDragSelect({
  scrollSelector,
  onStart,
  onSpan,
}: {
  /** The scrolling list the rows live in, scrolled near its edges. */
  scrollSelector: string;
  /** A slide began on row `index` of `list`. */
  onStart: (list: string, index: number) => void;
  /** The slide now covers rows `anchor` through `current`. */
  onSpan: (anchor: number, current: number) => void;
}) {
  const gesture = useRef<Gesture | null>(null);
  const skipClick = useRef(false);
  const [holding, setHolding] = useState<{
    list: string;
    index: number;
  } | null>(null);
  // Window listeners are added once per gesture, so they reach the latest
  // callbacks through a ref.
  const handlers = useRef({ onStart, onSpan });
  handlers.current = { onStart, onSpan };

  const rowAt = (g: Gesture) => {
    const hit = document
      .elementFromPoint(g.x, g.y)
      ?.closest<HTMLElement>("[data-drag-index]");
    if (!hit || hit.dataset.dragList !== g.list) return null;
    const index = Number(hit.dataset.dragIndex);
    return Number.isFinite(index) ? index : null;
  };

  const trace = (g: Gesture) => {
    const index = rowAt(g);
    if (index === null || index === g.last) return;
    g.last = index;
    handlers.current.onSpan(g.anchor, index);
  };

  // Keeps scrolling while the pointer rests near an edge, picking up the
  // rows that slide under it.
  const autoScroll = (g: Gesture) => {
    if (gesture.current !== g || !g.sliding) return;
    const scroll = g.scroll;
    if (scroll) {
      const box = scroll.getBoundingClientRect();
      let step = 0;
      if (g.y < box.top + EDGE_PX) step = -(box.top + EDGE_PX - g.y);
      else if (g.y > box.bottom - EDGE_PX) step = g.y - (box.bottom - EDGE_PX);
      if (step !== 0) {
        scroll.scrollTop += Math.sign(step) * Math.max(4, Math.abs(step) * 0.3);
        trace(g);
      }
    }
    g.frame = requestAnimationFrame(() => autoScroll(g));
  };

  const finish = () => {
    const g = gesture.current;
    if (!g) return;
    gesture.current = null;
    window.clearTimeout(g.timer);
    cancelAnimationFrame(g.frame);
    g.detach();
    setHolding(null);
  };

  const slide = (g: Gesture) => {
    window.clearTimeout(g.timer);
    g.sliding = true;
    g.last = g.anchor;
    // The release lands on a row; it must not toggle that row back.
    skipClick.current = true;
    setHolding(null);
    handlers.current.onStart(g.list, g.anchor);
    handlers.current.onSpan(g.anchor, g.anchor);
    navigator.vibrate?.(12);
    g.frame = requestAnimationFrame(() => autoScroll(g));
  };

  const move = (x: number, y: number) => {
    const g = gesture.current;
    if (!g) return;
    g.x = x;
    g.y = y;
    if (g.sliding) {
      trace(g);
      return;
    }
    if (Math.hypot(x - g.startX, y - g.startY) <= SLOP_PX) return;
    // A mouse slides at once. A finger that moves before the hold is up is
    // scrolling the list.
    if (g.mouse) {
      slide(g);
      trace(g);
    } else {
      finish();
    }
  };

  function onPointerMove(event: globalThis.PointerEvent) {
    if (event.pointerId === gesture.current?.pointerId) {
      move(event.clientX, event.clientY);
    }
  }

  function onPointerUp(event: globalThis.PointerEvent) {
    if (event.pointerId === gesture.current?.pointerId) finish();
  }

  // Once a slide has started on a touch screen, the browser may cancel the
  // pointer while the finger stays down; the touch events carry on, so they
  // end the slide instead.
  function onPointerCancel(event: globalThis.PointerEvent) {
    const g = gesture.current;
    if (!g || event.pointerId !== g.pointerId) return;
    if (g.sliding && !g.mouse) return;
    finish();
  }

  function onTouchMove(event: TouchEvent) {
    const g = gesture.current;
    const touch = event.touches[0];
    if (!g || !touch) return;
    // A slide moves the selection, not the list.
    if (g.sliding && event.cancelable) event.preventDefault();
    move(touch.clientX, touch.clientY);
  }

  // A gesture still going when the list closes leaves no listeners behind.
  useEffect(() => finish, []);

  const rowProps = (list: string, index: number) => ({
    "data-drag-list": list,
    "data-drag-index": index,
    onContextMenu: (event: { preventDefault: () => void }) =>
      event.preventDefault(),
    onPointerDown: (event: PointerEvent<HTMLElement>) => {
      if (event.button > 0 || gesture.current) return;
      skipClick.current = false;
      const mouse = event.pointerType === "mouse";
      const g: Gesture = {
        list,
        anchor: index,
        pointerId: event.pointerId,
        mouse,
        startX: event.clientX,
        startY: event.clientY,
        x: event.clientX,
        y: event.clientY,
        timer: 0,
        scroll: event.currentTarget.closest<HTMLElement>(scrollSelector),
        sliding: false,
        frame: 0,
        last: index,
        detach: () => {},
      };
      gesture.current = g;
      if (!mouse) {
        g.timer = window.setTimeout(() => {
          if (gesture.current === g) slide(g);
        }, HOLD_MS);
        setHolding({ list, index });
      }
      // These exact functions have to be the ones taken off again later.
      const end = () => finish();
      window.addEventListener("pointermove", onPointerMove);
      window.addEventListener("pointerup", onPointerUp);
      window.addEventListener("pointercancel", onPointerCancel);
      window.addEventListener("touchmove", onTouchMove, { passive: false });
      window.addEventListener("touchend", end);
      window.addEventListener("touchcancel", end);
      g.detach = () => {
        window.removeEventListener("pointermove", onPointerMove);
        window.removeEventListener("pointerup", onPointerUp);
        window.removeEventListener("pointercancel", onPointerCancel);
        window.removeEventListener("touchmove", onTouchMove);
        window.removeEventListener("touchend", end);
        window.removeEventListener("touchcancel", end);
      };
    },
  });

  /** True once for the click that ends a slide, which should be ignored. */
  const consumeClick = () => {
    const skip = skipClick.current;
    skipClick.current = false;
    return skip;
  };

  const isHolding = (list: string, index: number) =>
    holding?.list === list && holding.index === index;

  return { rowProps, consumeClick, isHolding };
}
