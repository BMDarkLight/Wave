import {
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

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
 * Flips above the trigger when there isn't enough room below.
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

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;

    const pad = 8;
    const rect = el.getBoundingClientRect();
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
      if (left + rect.width > vw - pad) left = Math.max(pad, vw - pad - rect.width);
    }

    setStyle({
      position: "fixed",
      top: `${Math.round(top)}px`,
      left: `${Math.round(left)}px`,
      right: "auto",
    });
  }, [anchor, bottomSafe]);

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
        className={className}
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
