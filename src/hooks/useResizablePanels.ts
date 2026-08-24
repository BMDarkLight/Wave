import { useEffect, useRef, useState } from "react";

/** Sidebar/right-panel widths, drag-to-resize, and the delayed-close
 * animation used when the right panel (queue/lyrics/device list) closes. */
export function useResizablePanels({
  panelOpen,
  onCloseQueue,
  onCloseDeviceList,
  onCloseLyrics,
}: {
  panelOpen: boolean;
  onCloseQueue: () => void;
  onCloseDeviceList: () => void;
  onCloseLyrics: () => void;
}) {
  const [sidebarWidth, setSidebarWidth] = useState(252);
  const [rightPanelWidth, setRightPanelWidth] = useState(320);
  const [rightPanelClosing, setRightPanelClosing] = useState(false);
  const rightPanelClosingRef = useRef(false);
  const rightPanelCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [dragging, setDragging] = useState<"sidebar" | "right" | null>(null);
  const dragStartRef = useRef({ x: 0, width: 0 });

  const isMobileLayout = () => window.innerWidth <= 900;

  const closeAllPanels = () => {
    onCloseQueue();
    onCloseDeviceList();
    onCloseLyrics();
  };

  const closeRightPanelDelayed = () => {
    if (rightPanelClosingRef.current) return;
    if (!isMobileLayout()) {
      closeAllPanels();
      return;
    }
    rightPanelClosingRef.current = true;
    setRightPanelClosing(true);
    rightPanelCloseTimer.current = setTimeout(() => {
      rightPanelClosingRef.current = false;
      setRightPanelClosing(false);
      closeAllPanels();
    }, 280);
  };

  const cancelCloseRightPanel = () => {
    if (rightPanelCloseTimer.current) {
      clearTimeout(rightPanelCloseTimer.current);
      rightPanelCloseTimer.current = null;
    }
    if (rightPanelClosingRef.current) {
      rightPanelClosingRef.current = false;
      setRightPanelClosing(false);
    }
  };

  const clampRightPanelWidth = (width: number, sidebar = sidebarWidth) => {
    const reserved = sidebar + 24 + 340; // resize gutters + minimum main column
    const max = Math.max(280, Math.min(400, window.innerWidth - reserved));
    return Math.max(280, Math.min(max, width));
  };

  useEffect(() => {
    const clamp = () => setRightPanelWidth((width) => clampRightPanelWidth(width));
    clamp();
    window.addEventListener("resize", clamp);
    return () => window.removeEventListener("resize", clamp);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sidebarWidth]);

  useEffect(() => {
    if (!dragging) return;
    const onMouseMove = (e: MouseEvent) => {
      const dx = e.clientX - dragStartRef.current.x;
      if (dragging === "sidebar") {
        setSidebarWidth(Math.max(180, Math.min(400, dragStartRef.current.width + dx)));
      } else {
        setRightPanelWidth(clampRightPanelWidth(dragStartRef.current.width - dx));
      }
    };
    const onMouseUp = () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.documentElement.style.userSelect = "";
      setDragging(null);
    };
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp, { once: true });
    return () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.documentElement.style.userSelect = "";
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dragging]);

  const onDragStart = (which: "sidebar" | "right") => (e: React.MouseEvent) => {
    e.preventDefault();
    dragStartRef.current = {
      x: e.clientX,
      width: which === "sidebar" ? sidebarWidth : rightPanelWidth,
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    document.documentElement.style.userSelect = "none";
    setDragging(which);
  };

  return {
    sidebarWidth,
    setSidebarWidth,
    rightPanelWidth,
    setRightPanelWidth,
    rightPanelOpen: panelOpen,
    rightPanelClosing,
    isMobileLayout,
    closeRightPanelDelayed,
    cancelCloseRightPanel,
    clampRightPanelWidth,
    dragging,
    onDragStart,
  };
}
