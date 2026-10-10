/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// The search field that opens under a Library page's header on the narrow
// layout and narrows that page's list as you type. It grows out of the
// search button and fades in, and shrinks back when closed.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { BiSearch, BiX } from "react-icons/bi";
import { lessMotion } from "../utils/appearance";
import { useBackLayer } from "../utils/backLayers";

/** Matches the close transition in the stylesheet. */
const CLOSE_MS = 240;
/** How the content below moves: the field's own curve, open and close. */
const OPEN_SLIDE = `transform 0.3s cubic-bezier(0.32, 0.72, 0, 1)`;
const CLOSE_SLIDE = `transform ${CLOSE_MS}ms cubic-bezier(0.32, 0.72, 0, 1)`;

/** Everything after the field in the page, which it pushes down. */
const contentBelow = (wrap: HTMLElement | null): HTMLElement[] => {
  const below: HTMLElement[] = [];
  for (let el = wrap?.nextElementSibling; el; el = el.nextElementSibling) {
    if (el instanceof HTMLElement) below.push(el);
  }
  return below;
};

/**
 * How far the field pushes the content below it, read from where that
 * content sits with and without the field. Its height alone misses the
 * margins around it, which would leave the content to jump the rest of the
 * way when the slide ends.
 */
const pushedBy = (wrap: HTMLElement, below: HTMLElement[]): number => {
  const first = below[0];
  if (!first) return 0;
  const shown = first.getBoundingClientRect().top;
  const display = wrap.style.display;
  wrap.style.display = "none";
  const hidden = first.getBoundingClientRect().top;
  wrap.style.display = display;
  return shown - hidden;
};

const place = (els: HTMLElement[], transition: string, offset: number) => {
  for (const el of els) {
    el.style.transition = transition;
    el.style.transform = offset ? `translateY(${offset}px)` : "";
  }
};

export default function MobileFilterField({
  open,
  label,
  placeholder,
  query,
  onQueryChange,
  onClose,
}: {
  open: boolean;
  /** What is being searched, for screen readers: "Search albums". */
  label: string;
  placeholder: string;
  query: string;
  onQueryChange: (query: string) => void;
  /** Clears the query and hides the field. */
  onClose: () => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  // Stays mounted through the close so it can fold away.
  const [mounted, setMounted] = useState(open);
  const [expanded, setExpanded] = useState(false);
  useBackLayer(open, onClose, "page");

  // The field takes its space at once. The content below is moved back up
  // by that much with a transform and then eased down, and on close eased
  // back up before the field leaves, so the list slides without being laid
  // out again on every frame.
  useEffect(() => {
    if (open) {
      setMounted(true);
      return;
    }
    const wrap = wrapRef.current;
    const below = contentBelow(wrap);
    setExpanded(false);
    if (wrap && !lessMotion()) {
      // Clear any offset left from opening before measuring.
      place(below, "none", 0);
      place(below, CLOSE_SLIDE, -pushedBy(wrap, below));
    }
    const timer = window.setTimeout(() => {
      // Drop the field and the offset in the same step so nothing jumps.
      flushSync(() => setMounted(false));
      place(below, "", 0);
    }, CLOSE_MS);
    return () => window.clearTimeout(timer);
  }, [open]);

  // Once the closed field is in the page, lay it out so the browser has its
  // folded state, then open it in the same step so it transitions from
  // there. Animation frames would do the same but stall in a hidden tab.
  useLayoutEffect(() => {
    if (!open || !mounted || expanded) return;
    const wrap = wrapRef.current;
    if (wrap && !lessMotion()) {
      const below = contentBelow(wrap);
      place(below, "none", 0);
      place(below, "none", -pushedBy(wrap, below));
      wrap.getBoundingClientRect();
      place(below, OPEN_SLIDE, 0);
    }
    setExpanded(true);
  }, [open, mounted, expanded]);

  useEffect(() => {
    if (expanded) inputRef.current?.focus();
  }, [expanded]);

  if (!mounted) return null;

  return (
    <div
      ref={wrapRef}
      className={`mobile-filter-wrap${expanded ? " is-open" : ""}`}
    >
      <div className="msearch-field mobile-filter-field">
        <BiSearch className="library-search-icon" aria-hidden />
        <input
          ref={inputRef}
          className="library-search-input"
          type="search"
          placeholder={placeholder}
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") onClose();
          }}
          aria-label={label}
          autoComplete="off"
          spellCheck={false}
        />
        <button
          className="library-search-clear"
          type="button"
          onClick={() => {
            if (query) {
              onQueryChange("");
              inputRef.current?.focus();
            } else {
              onClose();
            }
          }}
          title={query ? "Clear search" : "Close search"}
          aria-label={query ? "Clear search" : "Close search"}
        >
          <BiX />
        </button>
      </div>
    </div>
  );
}
