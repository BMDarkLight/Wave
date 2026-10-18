/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState, type ReactNode } from "react";
import { BiRefresh } from "react-icons/bi";

/** How far the disc can travel, in pixels. */
const MAX_PULL = 140;
/** How far a pull has to go to refresh. */
const THRESHOLD = 80;
/** Where the disc rests while refreshing, measured like a pull. */
const REST = 64;
/** The disc's size, so it starts fully hidden above the page. */
const DISC = 40;
/** Movement before the gesture decides whether it is a pull at all. */
const SLOP = 8;
const MESSAGE_MS = 2000;

/** The nearest ancestor that scrolls vertically, or the document. */
function scrollParent(element: HTMLElement): HTMLElement {
  let node: HTMLElement | null = element.parentElement;
  while (node) {
    const overflow = getComputedStyle(node).overflowY;
    if (overflow === "auto" || overflow === "scroll") return node;
    node = node.parentElement;
  }
  return (document.scrollingElement as HTMLElement) ?? document.body;
}

/** A finger's travel turned into a pull that slows down as it grows. */
function rubberBand(dy: number): number {
  return MAX_PULL * (1 - 1 / ((dy * 0.55) / MAX_PULL + 1));
}

/**
 * Pull down from the top of a page to refresh it. A disc follows the finger
 * over the page, moved only with transforms on its own layer, so the page
 * never lays out again or re-renders while it is dragged.
 */
export default function PullToRefresh({
  onRefresh,
  refreshing,
  cooldownSeconds,
  children,
}: {
  onRefresh: () => void;
  refreshing: boolean;
  cooldownSeconds: number;
  children: ReactNode;
}) {
  const root = useRef<HTMLDivElement | null>(null);
  const disc = useRef<HTMLDivElement | null>(null);
  const icon = useRef<HTMLSpanElement | null>(null);
  const dragging = useRef(false);
  const [message, setMessage] = useState<string | null>(null);
  const latest = useRef({ onRefresh, refreshing, cooldownSeconds });

  useEffect(() => {
    latest.current = { onRefresh, refreshing, cooldownSeconds };
  }, [onRefresh, refreshing, cooldownSeconds]);

  useEffect(() => {
    if (!message) return;
    const timer = window.setTimeout(() => setMessage(null), MESSAGE_MS);
    return () => window.clearTimeout(timer);
  }, [message]);

  /** Puts the disc at a pull distance, animated or straight under a finger. */
  const place = (distance: number, animate: boolean) => {
    const node = disc.current;
    if (!node) return;
    node.classList.toggle("settling", animate);
    node.style.transform = `translate3d(0, ${distance - DISC}px, 0)`;
    node.style.opacity = String(Math.min(1, distance / (THRESHOLD * 0.6)));
    if (icon.current && !animate) {
      icon.current.style.transform = `rotate(${(distance / THRESHOLD) * 270}deg)`;
    }
    node.classList.toggle("ready", distance >= THRESHOLD);
  };

  // A refresh started anywhere, or a message, holds the disc in view; when
  // both are over it slides away.
  useEffect(() => {
    if (dragging.current) return;
    place(refreshing || message ? REST : 0, true);
  }, [refreshing, message]);

  useEffect(() => {
    const element = root.current;
    if (!element) return;
    const scroller = scrollParent(element);
    let startX = 0;
    let startY: number | null = null;
    let decided = false;
    let distance = 0;
    let frame = 0;

    const draw = () => {
      frame = 0;
      place(distance, false);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(draw);
    };

    const onStart = (event: TouchEvent) => {
      const { refreshing } = latest.current;
      if (scroller.scrollTop > 0 || refreshing || event.touches.length > 1) {
        startY = null;
        return;
      }
      startX = event.touches[0].clientX;
      startY = event.touches[0].clientY;
      decided = false;
      distance = 0;
    };
    const onMove = (event: TouchEvent) => {
      if (startY === null) return;
      const dx = event.touches[0].clientX - startX;
      const dy = event.touches[0].clientY - startY;
      if (!decided) {
        if (Math.abs(dx) < SLOP && Math.abs(dy) < SLOP) return;
        // A sideways swipe (a shelf of albums) or an upward one is not a pull.
        if (Math.abs(dx) > Math.abs(dy) || dy < 0 || scroller.scrollTop > 0) {
          startY = null;
          return;
        }
        decided = true;
        dragging.current = true;
      }
      // Only a downward pull from the very top takes over the gesture.
      event.preventDefault();
      distance = rubberBand(Math.max(0, dy - SLOP));
      schedule();
    };
    const onEnd = () => {
      if (startY === null || !decided) {
        startY = null;
        return;
      }
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      dragging.current = false;
      startY = null;
      const { onRefresh, refreshing, cooldownSeconds } = latest.current;
      if (distance >= THRESHOLD && !refreshing) {
        if (cooldownSeconds > 0) {
          setMessage(`Refreshed recently. Try again in ${cooldownSeconds}s.`);
        } else {
          onRefresh();
        }
        place(REST, true);
      } else {
        place(refreshing ? REST : 0, true);
      }
      distance = 0;
    };

    scroller.addEventListener("touchstart", onStart, { passive: true });
    scroller.addEventListener("touchmove", onMove, { passive: false });
    scroller.addEventListener("touchend", onEnd, { passive: true });
    scroller.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      if (frame) cancelAnimationFrame(frame);
      scroller.removeEventListener("touchstart", onStart);
      scroller.removeEventListener("touchmove", onMove);
      scroller.removeEventListener("touchend", onEnd);
      scroller.removeEventListener("touchcancel", onEnd);
    };
  }, []);

  return (
    <div ref={root} className="ptr-root">
      <div className="ptr-track" aria-hidden={!refreshing && !message}>
        <div
          ref={disc}
          className={`ptr-disc${message ? " has-message" : ""}`}
          role="status"
          aria-live="polite"
        >
          {message ? (
            <span className="ptr-message">{message}</span>
          ) : refreshing ? (
            <span className="spinner" aria-label="Refreshing" />
          ) : (
            <span ref={icon} className="ptr-icon">
              <BiRefresh />
            </span>
          )}
        </div>
      </div>
      {children}
    </div>
  );
}
