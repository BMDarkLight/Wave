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
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const [scrollMargin, setScrollMargin] = useState(0);

  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const scroll = list.closest(scrollSelector) as HTMLElement | null;
    setScrollElement(scroll);
  }, [scrollSelector]);

  const measureMargin = useCallback(() => {
    const list = listRef.current;
    const scroll = scrollElement;
    if (!list || !scroll) return;
    const listTop = list.getBoundingClientRect().top;
    const scrollTop = scroll.getBoundingClientRect().top;
    setScrollMargin(listTop - scrollTop + scroll.scrollTop);
  }, [scrollElement]);

  useLayoutEffect(() => {
    measureMargin();
    const list = listRef.current;
    const scroll = scrollElement;
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
  }, [scrollElement, measureMargin, count]);

  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollElement,
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
