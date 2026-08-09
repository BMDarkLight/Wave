import {
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";

type UseDragDismissOptions = {
  onDismiss: () => void;
  enabled?: boolean;
  /** Downward travel (px) required to dismiss on release. */
  threshold?: number;
  /** Downward velocity (px/ms) that dismisses even below the travel threshold. */
  velocityThreshold?: number;
};

/**
 * Pointer-driven downward swipe-to-dismiss for full-screen sheets / pages.
 * Bind the returned handlers to a drag surface (header, handle, cover).
 *
 * Callers should arm a short "ignore reopen" window inside `onDismiss` so the
 * Android WebView's delayed synthetic click (often ~200–300ms later) cannot
 * immediately reopen the surface that just closed.
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
  const movedRef = useRef(false);
  const onDismissRef = useRef(onDismiss);
  const enabledRef = useRef(enabled);
  onDismissRef.current = onDismiss;
  enabledRef.current = enabled;

  const reset = () => {
    activeRef.current = false;
    offsetRef.current = 0;
    velocityRef.current = 0;
    movedRef.current = false;
    setDragging(false);
    setOffset(0);
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLElement>) => {
    if (!enabledRef.current) return;
    if (e.button !== 0) return;
    const target = e.target as HTMLElement | null;
    if (target?.closest("button, a, input, select, textarea, label")) return;

    activeRef.current = true;
    movedRef.current = false;
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
    if (next > 8) movedRef.current = true;
    offsetRef.current = next;
    setOffset(next);
  };

  const finish = (event?: ReactPointerEvent<HTMLElement>) => {
    if (!activeRef.current) return;
    const shouldDismiss =
      offsetRef.current >= threshold || velocityRef.current >= velocityThreshold;
    activeRef.current = false;
    setDragging(false);
    if (shouldDismiss) {
      // Prevent the compatibility mouse/click synthesized from this gesture
      // when the browser still delivers them on the same target.
      event?.preventDefault?.();
      onDismissRef.current();
      requestAnimationFrame(() => {
        offsetRef.current = 0;
        movedRef.current = false;
        setOffset(0);
      });
      return;
    }
    offsetRef.current = 0;
    movedRef.current = false;
    setOffset(0);
  };

  const onPointerUp = (e: ReactPointerEvent<HTMLElement>) => finish(e);
  const onPointerCancel = () => reset();

  // Swallow the ghost click that follows a drag-dismiss on the drag surface
  // itself (sheet handle / header). Player-bar reopens are guarded separately.
  const onClick = (e: ReactMouseEvent<HTMLElement>) => {
    if (movedRef.current) {
      e.preventDefault();
      e.stopPropagation();
      movedRef.current = false;
    }
  };

  return {
    offset,
    dragging,
    bind: {
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onPointerCancel,
      onClick,
    },
  };
}

/** How long to ignore reopen after a drag-dismiss (Android delayed click). */
export const DRAG_DISMISS_REOPEN_GUARD_MS = 380;
