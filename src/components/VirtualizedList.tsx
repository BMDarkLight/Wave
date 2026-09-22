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
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

type VirtualizedListProps = {
  count: number;
  /** Estimated row height in px (used before measure). */
  estimateSize?: number;
  overscan?: number;
  className?: string;
  style?: CSSProperties;
  /** Selector for the nearest scroll parent (defaults to `.main-content`). */
  scrollSelector?: string;
  children: (index: number) => ReactNode;
};

/**
 * Windowed list for long playlists. Only mounts rows near the viewport so
 * 2k–3k track lists stay scrollable without mounting every row.
 *
 * Expects to live inside a scrolling ancestor (`.main-content` by default),
 * with optional sticky headers rendered as siblings above this component.
 */
export default function VirtualizedList({
  count,
  estimateSize = 64,
  overscan = 14,
  className,
  style,
  scrollSelector = ".main-content",
  children,
}: VirtualizedListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLElement | null>(null);
  const [scrollMargin, setScrollMargin] = useState(0);

  const getScrollElement = useCallback(() => {
    if (!scrollRef.current) {
      scrollRef.current = listRef.current?.closest(
        scrollSelector,
      ) as HTMLElement | null;
    }
    return scrollRef.current;
  }, [scrollSelector]);

  /**
   * The scroll parent can only be found once this list is in the document, so
   * the virtualizer stays off for the first render. Running it before then
   * makes it record a starting offset of zero and write that back to the page
   * as soon as it attaches, which threw the reader to the top whenever a list
   * mounted into an already-scrolled page.
   */
  const [attached, setAttached] = useState(false);

  useLayoutEffect(() => {
    scrollRef.current = null;
    setAttached(!!getScrollElement());
  }, [scrollSelector, getScrollElement]);

  const measureMargin = useCallback(() => {
    const list = listRef.current;
    const scroll = getScrollElement();
    if (!list || !scroll) return;
    const listTop = list.getBoundingClientRect().top;
    const scrollTop = scroll.getBoundingClientRect().top;
    setScrollMargin(listTop - scrollTop + scroll.scrollTop);
  }, [getScrollElement]);

  useLayoutEffect(() => {
    measureMargin();
    const list = listRef.current;
    const scroll = getScrollElement();
    if (!list || !scroll) return;

    const ro = new ResizeObserver(() => measureMargin());
    ro.observe(scroll);
    // Hero / playlist header height changes also move the list.
    const hero = scroll.querySelector(".hero-copy, .album-hero, .artist-hero");
    if (hero) ro.observe(hero);

    window.addEventListener("resize", measureMargin);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measureMargin);
    };
  }, [getScrollElement, measureMargin, count]);

  const virtualizer = useVirtualizer({
    count,
    enabled: attached,
    getScrollElement,
    initialOffset: () => getScrollElement()?.scrollTop ?? 0,
    estimateSize: () => estimateSize,
    overscan,
    scrollMargin,
  });

  const items = virtualizer.getVirtualItems();

  return (
    <div
      ref={listRef}
      className={className}
      style={{
        ...style,
        height: `${virtualizer.getTotalSize()}px`,
        width: "100%",
        position: "relative",
      }}
    >
      {items.map((item) => (
        <div
          key={item.key}
          data-index={item.index}
          ref={virtualizer.measureElement}
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: "100%",
            transform: `translateY(${item.start - scrollMargin}px)`,
          }}
        >
          {children(item.index)}
        </div>
      ))}
    </div>
  );
}
