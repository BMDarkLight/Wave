/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import {
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { lessMotion } from "../utils/appearance";

/** Matches the close transition in the stylesheet. */
const CLOSE_MS = 180;

export type ContextMenuAnchor = {
  /** Preferred top edge (usually below the trigger). */
  top: number;
  left?: number;
  right?: number;
  /** When flipping above the trigger, align the menu bottom to this Y. */
  flipAbove?: number;
};

type ContextMenuProps = {
  anchor: ContextMenuAnchor;
  onClose: () => void;
  children: ReactNode;
  className?: string;
  /** Extra bottom inset (player bar, etc.). */
  bottomSafe?: number;
};

/**
 * Fixed, portaled context menu that stays inside the viewport.
 * Flips above the trigger when there isn't enough room below. It grows out
 * of the point it was opened from and fades in, and shrinks back when it
 * closes.
 */
export default function ContextMenu({
  anchor,
  onClose,
  children,
  className = "track-context-menu",
  bottomSafe = 110,
}: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>(() => initialStyle(anchor));
  const [open, setOpen] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;

    const pad = 8;
    // The layout size, not getBoundingClientRect: the menu starts scaled down
    // for its opening, and measuring that smaller box would let the full-size
    // menu run past the screen edge.
    const rect = { width: el.offsetWidth, height: el.offsetHeight };
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const maxBottom = vh - Math.max(pad, bottomSafe);

    let top = anchor.top;
    if (top + rect.height > maxBottom) {
      const flipFrom = anchor.flipAbove ?? anchor.top;
      top = flipFrom - rect.height;
    }
    top = Math.min(Math.max(pad, top), Math.max(pad, maxBottom - rect.height));

    let left: number | undefined;
    if (anchor.left != null) {
      left = anchor.left;
      if (left + rect.width > vw - pad) left = vw - pad - rect.width;
      left = Math.max(pad, left);
    } else {
      const right = anchor.right ?? pad;
      left = vw - right - rect.width;
      if (left < pad) left = pad;
      if (left + rect.width > vw - pad)
        left = Math.max(pad, vw - pad - rect.width);
    }

    // Grow from the point the menu was opened from: the click, or the edge
    // of the button it hangs off.
    const pointX = anchor.left ?? vw - (anchor.right ?? pad);
    const flipped = top < anchor.top;
    const originX = Math.min(Math.max(0, pointX - left), rect.width);
    const originY = flipped
      ? rect.height
      : Math.min(Math.max(0, anchor.top - top), rect.height);

    setStyle({
      position: "fixed",
      top: `${Math.round(top)}px`,
      left: `${Math.round(left)}px`,
      right: "auto",
      transformOrigin: `${Math.round(originX)}px ${Math.round(originY)}px`,
    });
    // The closed state was laid out by the measurement above, so opening now
    // transitions from it.
    setOpen(true);
  }, [anchor, bottomSafe]);

  // Callers remove the menu as soon as it closes, so a copy of it, which
  // takes no clicks, plays the close on its own. The copy only goes up once
  // the menu has really left the page, not when development mode unmounts
  // and remounts it to test effects.
  useLayoutEffect(() => {
    const menu = ref.current;
    return () => {
      if (!menu || lessMotion()) return;
      const ghost = menu.cloneNode(true) as HTMLElement;
      ghost.setAttribute("aria-hidden", "true");
      ghost.classList.add("context-menu-ghost");
      queueMicrotask(() => {
        if (menu.isConnected) return;
        document.body.appendChild(ghost);
        ghost.getBoundingClientRect();
        ghost.classList.remove("context-menu-open");
        window.setTimeout(() => ghost.remove(), CLOSE_MS);
      });
    };
  }, []);

  return createPortal(
    <>
      <div
        className="context-menu-backdrop"
        onClick={onClose}
        onContextMenu={(event) => {
          event.preventDefault();
          onClose();
        }}
      />
      <div
        ref={ref}
        className={`context-menu ${className}${open ? " context-menu-open" : ""}`}
        style={style}
        role="menu"
        onClick={(event) => event.stopPropagation()}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}

function initialStyle(anchor: ContextMenuAnchor): CSSProperties {
  if (anchor.left != null) {
    return {
      position: "fixed",
      top: `${anchor.top}px`,
      left: `${anchor.left}px`,
    };
  }
  return {
    position: "fixed",
    top: `${anchor.top}px`,
    right: `${anchor.right ?? 0}px`,
  };
}
