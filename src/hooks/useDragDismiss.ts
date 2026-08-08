import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

type UseDragDismissOptions = {
  onDismiss: () => void;
  enabled?: boolean;
  /** Downward travel (px) required to dismiss on release. */
  threshold?: number;
  /** Downward velocity (px/ms) that dismisses even below the travel threshold. */
  velocityThreshold?: number;
};

/**
 * After a successful drag-dismiss, the browser still synthesizes a `click` on
 * whatever is under the finger (often the mini player bar). Swallow only that
 * immediate ghost click so it cannot reopen the sheet — keep the window short
 * so a real follow-up tap still works.
 */
function suppressTrailingClick(durationMs = 80) {
  let armed = true;
  const suppress = (event: Event) => {
    if (!armed) return;
    armed = false;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    window.removeEventListener("click", suppress, true);
  };
  window.addEventListener("click", suppress, true);
  window.setTimeout(() => {
    armed = false;
    window.removeEventListener("click", suppress, true);
  }, durationMs);
}

/**
 * Pointer-driven downward swipe-to-dismiss for full-screen sheets / pages.
 * Bind the returned handlers to a drag surface (header, handle, cover).
 */
export function useDragDismiss({
  onDismiss,
  enabled = true,
  threshold = 110,
  velocityThreshold = 0.55,
}: UseDragDismissOptions) {
  const [offset, setOffset] = useState(0);
  const [dragging, setDragging] = useState(false);
  const activeRef = useRef(false);
  const startYRef = useRef(0);
  const lastYRef = useRef(0);
  const lastTRef = useRef(0);
  const offsetRef = useRef(0);
  const velocityRef = useRef(0);
  const onDismissRef = useRef(onDismiss);
  const enabledRef = useRef(enabled);
  onDismissRef.current = onDismiss;
  enabledRef.current = enabled;

  const reset = () => {
    activeRef.current = false;
    offsetRef.current = 0;
    velocityRef.current = 0;
    setDragging(false);
    setOffset(0);
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLElement>) => {
    if (!enabledRef.current) return;
    if (e.button !== 0) return;
    const target = e.target as HTMLElement | null;
    if (target?.closest("button, a, input, select, textarea, label")) return;

    activeRef.current = true;
    startYRef.current = e.clientY;
    lastYRef.current = e.clientY;
    lastTRef.current = performance.now();
    offsetRef.current = 0;
    velocityRef.current = 0;
    setDragging(true);
    setOffset(0);
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLElement>) => {
    if (!activeRef.current) return;
    const now = performance.now();
    const dy = e.clientY - lastYRef.current;
    const dt = Math.max(1, now - lastTRef.current);
    velocityRef.current = dy / dt;
    lastYRef.current = e.clientY;
    lastTRef.current = now;
    const next = Math.max(0, e.clientY - startYRef.current);
    offsetRef.current = next;
    setOffset(next);
  };

  const finish = () => {
    if (!activeRef.current) return;
    const shouldDismiss =
      offsetRef.current >= threshold || velocityRef.current >= velocityThreshold;
    activeRef.current = false;
    setDragging(false);
    if (shouldDismiss) {
      // Arm before onDismiss: dismiss sets pointer-events:none, so the
      // trailing click would otherwise hit the player bar and reopen NP.
      suppressTrailingClick();
      onDismissRef.current();
      // Let the close animation own transform; drop the drag offset next frame.
      requestAnimationFrame(() => {
        offsetRef.current = 0;
        setOffset(0);
      });
      return;
    }
    offsetRef.current = 0;
    setOffset(0);
  };

  const onPointerUp = () => finish();
  const onPointerCancel = () => reset();

  return {
    offset,
    dragging,
    bind: {
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onPointerCancel,
    },
  };
}
