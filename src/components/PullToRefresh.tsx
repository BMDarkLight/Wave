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
import { BiDownArrowAlt } from "react-icons/bi";

const RESISTANCE = 0.5;
const THRESHOLD = 64;
const MAX_PULL = 120;
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

/** Pull down from the top of a page to refresh it. */
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
  const [pull, setPull] = useState(0);
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

  useEffect(() => {
    const element = root.current;
    if (!element) return;
    const scroller = scrollParent(element);
    let startY: number | null = null;
    let distance = 0;

    const onStart = (event: TouchEvent) => {
      startY = scroller.scrollTop <= 0 ? event.touches[0].clientY : null;
      distance = 0;
    };
    const onMove = (event: TouchEvent) => {
      if (startY === null) return;
      const dy = event.touches[0].clientY - startY;
      if (dy <= 0 || scroller.scrollTop > 0) {
        if (distance > 0) setPull(0);
        distance = 0;
        return;
      }
      // Only a downward pull from the very top takes over the gesture.
      event.preventDefault();
      distance = Math.min(dy * RESISTANCE, MAX_PULL);
      setPull(distance);
    };
    const onEnd = () => {
      if (startY !== null && distance >= THRESHOLD) {
        const { onRefresh, refreshing, cooldownSeconds } = latest.current;
        if (cooldownSeconds > 0 && !refreshing) {
          setMessage(`Refreshed recently. Try again in ${cooldownSeconds}s.`);
        } else if (!refreshing) {
          onRefresh();
        }
      }
      startY = null;
      distance = 0;
      setPull(0);
    };

    scroller.addEventListener("touchstart", onStart, { passive: true });
    scroller.addEventListener("touchmove", onMove, { passive: false });
    scroller.addEventListener("touchend", onEnd, { passive: true });
    scroller.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      scroller.removeEventListener("touchstart", onStart);
      scroller.removeEventListener("touchmove", onMove);
      scroller.removeEventListener("touchend", onEnd);
      scroller.removeEventListener("touchcancel", onEnd);
    };
  }, []);

  const showIndicator = refreshing || pull > 0 || message !== null;
  return (
    <div ref={root} className="ptr-root">
      {showIndicator && (
        <div
          className="ptr-indicator"
          style={{ height: refreshing || message ? THRESHOLD : pull }}
          role="status"
          aria-live="polite"
        >
          {refreshing ? (
            <span className="spinner" aria-label="Refreshing" />
          ) : message ? (
            <span className="ptr-message">{message}</span>
          ) : (
            <BiDownArrowAlt
              className={`ptr-arrow ${pull >= THRESHOLD ? "ready" : ""}`}
            />
          )}
        </div>
      )}
      {children}
    </div>
  );
}
