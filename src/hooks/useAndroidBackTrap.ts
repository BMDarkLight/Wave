import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { exitApp } from "../utils/player";

type OverlaySnapshot = {
  showFolderSetup: boolean;
  menuTrackPath: string | null;
  queueMenuIndex: number | null;
  showAddTrackMenu: boolean;
  showEqPanel: boolean;
  mobilePlayerMenuOpen: boolean;
  playlistDialog: unknown;
  showClearConfirm: boolean;
  showAddFromLibrary: boolean;
  deletePlaylistConfirm: unknown;
  addToPlaylistTrack: string | null;
  mobilePlayerSubView: boolean;
  rightPanelOpen: boolean;
  mobilePlayerOpen: boolean;
  mobileSettingsOpen: boolean;
  mobileNavOpen: boolean;
  mainSearchOpen: boolean;
  browseDepth: number;
};

/** Android hardware back-button trap: maintains a synthetic browser-history
 * stack matching the number of open overlay "layers" so hardware back closes
 * the topmost one (context menus first, then sheets/dialogs, then Now
 * Playing subviews, then panels, then nav/search/browse), and shows a
 * double-back-to-exit toast once nothing is left to close. */
export function useAndroidBackTrap({
  androidHost,
  androidHostRef,
  showFolderSetup,
  skipFolderSetup,
  menuTrackPath,
  closeTrackContextMenu,
  queueMenuIndex,
  closeQueueContextMenu,
  showAddTrackMenu,
  setShowAddTrackMenu,
  setAddTrackMenuAnchor,
  showEqPanel,
  setShowEqPanel,
  setEqAnchor,
  mobilePlayerMenuOpen,
  setMobilePlayerMenuOpen,
  playlistDialog,
  closePlaylistDialog,
  showClearConfirm,
  setShowClearConfirm,
  showAddFromLibrary,
  closeAddFromLibrary,
  deletePlaylistConfirm,
  setDeletePlaylistConfirm,
  addToPlaylistTrack,
  setAddToPlaylistTrack,
  mobilePlayerView,
  setMobilePlayerView,
  rightPanelOpen,
  cancelCloseRightPanel,
  setShowQueue,
  setShowDeviceList,
  setLyricsPanelTrack,
  mobileSettingsOpen,
  mobileSettingsClosing,
  mobileSettingsClosingRef,
  forceCloseMobileSettings,
  handleCloseMobileSettings,
  mobilePlayerOpen,
  mobilePlayerClosing,
  mobilePlayerClosingRef,
  forceCloseMobilePlayer,
  handleCloseMobilePlayer,
  mobileNavOpen,
  setMobileNavOpen,
  mainSearchOpen,
  closeMainSearch,
  browseStackLength,
  browseBack,
}: {
  androidHost: boolean;
  androidHostRef: { current: boolean };
  showFolderSetup: boolean;
  skipFolderSetup: () => void | Promise<void>;
  menuTrackPath: string | null;
  closeTrackContextMenu: () => void;
  queueMenuIndex: number | null;
  closeQueueContextMenu: () => void;
  showAddTrackMenu: boolean;
  setShowAddTrackMenu: (show: boolean) => void;
  setAddTrackMenuAnchor: (anchor: null) => void;
  showEqPanel: boolean;
  setShowEqPanel: (show: boolean) => void;
  setEqAnchor: (anchor: null) => void;
  mobilePlayerMenuOpen: boolean;
  setMobilePlayerMenuOpen: (open: boolean) => void;
  playlistDialog: unknown;
  closePlaylistDialog: () => void;
  showClearConfirm: boolean;
  setShowClearConfirm: (show: boolean) => void;
  showAddFromLibrary: boolean;
  closeAddFromLibrary: () => void;
  deletePlaylistConfirm: unknown;
  setDeletePlaylistConfirm: (v: null) => void;
  addToPlaylistTrack: string | null;
  setAddToPlaylistTrack: (v: string | null) => void;
  mobilePlayerView: string;
  setMobilePlayerView: (view: "cover") => void;
  rightPanelOpen: boolean;
  cancelCloseRightPanel: () => void;
  setShowQueue: (show: boolean) => void;
  setShowDeviceList: (show: boolean) => void;
  setLyricsPanelTrack: (track: null) => void;
  mobileSettingsOpen: boolean;
  mobileSettingsClosing: boolean;
  mobileSettingsClosingRef: { current: boolean };
  forceCloseMobileSettings: () => void;
  handleCloseMobileSettings: () => void;
  mobilePlayerOpen: boolean;
  mobilePlayerClosing: boolean;
  mobilePlayerClosingRef: { current: boolean };
  forceCloseMobilePlayer: () => void;
  handleCloseMobilePlayer: () => void;
  mobileNavOpen: boolean;
  setMobileNavOpen: (open: boolean) => void;
  mainSearchOpen: boolean;
  closeMainSearch: () => void;
  browseStackLength: number;
  browseBack: () => void;
}) {
  const overlaySnapshotRef = useRef<OverlaySnapshot>({
    showFolderSetup,
    menuTrackPath,
    queueMenuIndex,
    showAddTrackMenu,
    showEqPanel,
    mobilePlayerMenuOpen,
    playlistDialog,
    showClearConfirm,
    showAddFromLibrary,
    deletePlaylistConfirm,
    addToPlaylistTrack,
    mobilePlayerSubView: mobilePlayerView !== "cover",
    rightPanelOpen,
    // Closing counts as dismissed so history can shrink during the exit anim.
    mobilePlayerOpen: mobilePlayerOpen && !mobilePlayerClosing,
    mobileSettingsOpen: mobileSettingsOpen && !mobileSettingsClosing,
    mobileNavOpen,
    mainSearchOpen,
    browseDepth: browseStackLength,
  });
  overlaySnapshotRef.current = {
    showFolderSetup,
    menuTrackPath,
    queueMenuIndex,
    showAddTrackMenu,
    showEqPanel,
    mobilePlayerMenuOpen,
    playlistDialog,
    showClearConfirm,
    showAddFromLibrary,
    deletePlaylistConfirm,
    addToPlaylistTrack,
    mobilePlayerSubView: mobilePlayerView !== "cover",
    rightPanelOpen,
    mobilePlayerOpen: mobilePlayerOpen && !mobilePlayerClosing,
    mobileSettingsOpen: mobileSettingsOpen && !mobileSettingsClosing,
    mobileNavOpen,
    mainSearchOpen,
    browseDepth: browseStackLength,
  };

  const countHistoryLayers = (s: OverlaySnapshot = overlaySnapshotRef.current) => {
    let layers = 0;
    if (s.showFolderSetup) layers++;
    if (s.menuTrackPath) layers++;
    if (s.queueMenuIndex != null) layers++;
    if (s.showAddTrackMenu) layers++;
    if (s.showEqPanel) layers++;
    if (s.playlistDialog) layers++;
    if (s.showClearConfirm) layers++;
    if (s.showAddFromLibrary) layers++;
    if (s.deletePlaylistConfirm) layers++;
    if (s.addToPlaylistTrack) layers++;
    if (s.mobilePlayerMenuOpen) layers++;
    if (s.mobilePlayerSubView) layers++;
    if (s.mobilePlayerOpen) layers++;
    if (s.mobileSettingsOpen) layers++;
    if (s.rightPanelOpen) layers++;
    if (s.mobileNavOpen) layers++;
    if (s.mainSearchOpen) layers++;
    layers += s.browseDepth;
    return layers;
  };

  const targetTrapDepth = () => {
    const layers = countHistoryLayers();
    return androidHostRef.current ? layers + 1 : layers;
  };

  // Closes whichever overlay is "on top" — context menus first, then sheets /
  // dialogs, then NP subviews, then panels, then nav/search/browse.
  // Also clears the matching snapshot field immediately so a rapid second back
  // (before React re-renders) still sees the updated stack.
  const closeTopOverlay = (): boolean => {
    const s = overlaySnapshotRef.current;
    if (s.showFolderSetup) {
      s.showFolderSetup = false;
      void skipFolderSetup();
      return true;
    }
    if (s.menuTrackPath) {
      s.menuTrackPath = null;
      closeTrackContextMenu();
      return true;
    }
    if (s.queueMenuIndex != null) {
      s.queueMenuIndex = null;
      closeQueueContextMenu();
      return true;
    }
    if (s.showAddTrackMenu) {
      s.showAddTrackMenu = false;
      setShowAddTrackMenu(false);
      setAddTrackMenuAnchor(null);
      return true;
    }
    if (s.showEqPanel) {
      s.showEqPanel = false;
      setShowEqPanel(false);
      setEqAnchor(null);
      return true;
    }
    if (s.mobilePlayerMenuOpen) {
      s.mobilePlayerMenuOpen = false;
      setMobilePlayerMenuOpen(false);
      return true;
    }
    if (s.playlistDialog) {
      s.playlistDialog = null;
      closePlaylistDialog();
      return true;
    }
    if (s.showClearConfirm) {
      s.showClearConfirm = false;
      setShowClearConfirm(false);
      return true;
    }
    if (s.showAddFromLibrary) {
      s.showAddFromLibrary = false;
      closeAddFromLibrary();
      return true;
    }
    if (s.deletePlaylistConfirm) {
      s.deletePlaylistConfirm = null;
      setDeletePlaylistConfirm(null);
      return true;
    }
    if (s.addToPlaylistTrack) {
      s.addToPlaylistTrack = null;
      setAddToPlaylistTrack(null);
      return true;
    }
    if (s.mobilePlayerSubView) {
      s.mobilePlayerSubView = false;
      setMobilePlayerView("cover");
      return true;
    }
    if (s.rightPanelOpen) {
      s.rightPanelOpen = false;
      cancelCloseRightPanel();
      setShowQueue(false);
      setShowDeviceList(false);
      setLyricsPanelTrack(null);
      return true;
    }
    // Settings is a sibling overlay to Now Playing — close it before the player
    // so a lone Settings page always pops first.
    if (mobileSettingsClosingRef.current) {
      s.mobileSettingsOpen = false;
      forceCloseMobileSettings();
      return true;
    }
    if (s.mobileSettingsOpen) {
      s.mobileSettingsOpen = false;
      handleCloseMobileSettings();
      return true;
    }
    // During the close animation snapshot already treats NP as gone; a second
    // back should force-unmount instead of falling through to exit toast.
    if (mobilePlayerClosingRef.current) {
      s.mobilePlayerOpen = false;
      forceCloseMobilePlayer();
      return true;
    }
    if (s.mobilePlayerOpen) {
      s.mobilePlayerOpen = false;
      handleCloseMobilePlayer();
      return true;
    }
    if (s.mobileNavOpen) {
      s.mobileNavOpen = false;
      setMobileNavOpen(false);
      return true;
    }
    if (s.mainSearchOpen) {
      s.mainSearchOpen = false;
      closeMainSearch();
      return true;
    }
    if (s.browseDepth > 0) {
      s.browseDepth -= 1;
      browseBack();
      return true;
    }
    return false;
  };

  const trapDepthRef = useRef(0);
  const ignorePopCountRef = useRef(0);
  const exitPressAtRef = useRef(0);
  const exitToastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [showExitToast, setShowExitToast] = useState(false);
  const closeTopOverlayRef = useRef(closeTopOverlay);
  closeTopOverlayRef.current = closeTopOverlay;
  const targetTrapDepthRef = useRef(targetTrapDepth);
  targetTrapDepthRef.current = targetTrapDepth;

  const clearExitPrompt = () => {
    exitPressAtRef.current = 0;
    setShowExitToast(false);
    if (exitToastTimerRef.current) {
      clearTimeout(exitToastTimerRef.current);
      exitToastTimerRef.current = null;
    }
  };

  const scheduleExitPromptExpiry = () => {
    if (exitToastTimerRef.current) {
      clearTimeout(exitToastTimerRef.current);
    }
    exitToastTimerRef.current = setTimeout(() => {
      exitPressAtRef.current = 0;
      setShowExitToast(false);
      exitToastTimerRef.current = null;
    }, 2100);
  };

  // Match synthetic history entries to open navigation layers (+ root guard).
  // Grow with pushState; shrink with history.go when UI dismisses overlays
  // without a hardware back (drag/chevron), so orphan entries don't skip the
  // double-back-to-exit toast.
  useLayoutEffect(() => {
    const target = targetTrapDepthRef.current();

    while (trapDepthRef.current < target) {
      window.history.pushState({ waveNav: true }, "");
      trapDepthRef.current += 1;
    }

    const excess = trapDepthRef.current - target;
    if (excess > 0) {
      // history.go(-n) emits a single popstate, not n.
      ignorePopCountRef.current += 1;
      trapDepthRef.current = target;
      window.history.go(-excess);
    }

    if (countHistoryLayers() > 0) {
      clearExitPrompt();
    }
  }, [
    showFolderSetup,
    menuTrackPath,
    queueMenuIndex,
    showAddTrackMenu,
    showEqPanel,
    mobilePlayerMenuOpen,
    playlistDialog,
    showClearConfirm,
    showAddFromLibrary,
    deletePlaylistConfirm,
    addToPlaylistTrack,
    mobilePlayerView,
    rightPanelOpen,
    mobilePlayerOpen,
    mobilePlayerClosing,
    mobileSettingsOpen,
    mobileSettingsClosing,
    mobileNavOpen,
    mainSearchOpen,
    browseStackLength,
    androidHost,
  ]);

  useEffect(() => {
    const onPopState = () => {
      if (ignorePopCountRef.current > 0) {
        ignorePopCountRef.current -= 1;
        return;
      }

      trapDepthRef.current = Math.max(0, trapDepthRef.current - 1);

      if (closeTopOverlayRef.current()) {
        clearExitPrompt();
        return;
      }

      if (!androidHostRef.current) {
        return;
      }

      const now = Date.now();
      if (exitPressAtRef.current > 0 && now - exitPressAtRef.current < 2000) {
        clearExitPrompt();
        // Confirm exit — the back press already consumed history; explicitly
        // leave the process so the user is not stuck needing a third back.
        void exitApp().catch((err) => {
          console.error("Failed to exit Wave:", err);
        });
        return;
      }

      exitPressAtRef.current = now;
      setShowExitToast(true);
      scheduleExitPromptExpiry();
      window.history.pushState({ waveExitGuard: true }, "");
      trapDepthRef.current += 1;
    };
    window.addEventListener("popstate", onPopState);
    return () => {
      window.removeEventListener("popstate", onPopState);
      if (exitToastTimerRef.current) {
        clearTimeout(exitToastTimerRef.current);
      }
    };
  }, []);

  return { showExitToast };
}
