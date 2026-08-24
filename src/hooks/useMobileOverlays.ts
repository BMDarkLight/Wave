import { useEffect, useRef, useState } from "react";
import { isAndroid } from "../utils/platform";
import { enableNoHoverMode } from "../utils/touchHover";
import { isLibraryPlaylistName } from "../utils/track";
import {
  dismissFolderSetup,
  isFolderSetupDismissed,
  listMediaFolders,
  type PlaylistInfo,
} from "../utils/player";
import type { MobileNowPlayingView } from "../components/MobileNowPlaying";
import type { MainView } from "../components/Sidebar";

/** Mobile-only overlay state: the nav drawer, the fullscreen Now Playing
 * page, the Settings overlay, Android host detection, and the first-run
 * "pick a music folder" prompt. */
export function useMobileOverlays({
  mobileNavOpen,
  setMobileNavOpen,
  androidHost,
  setAndroidHost,
  androidHostRef,
  playlists,
  setMainView,
  clearBrowse,
  onAndroidDetected,
}: {
  mobileNavOpen: boolean;
  setMobileNavOpen: (open: boolean) => void;
  androidHost: boolean;
  setAndroidHost: (android: boolean) => void;
  androidHostRef: { current: boolean };
  playlists: PlaylistInfo[];
  setMainView: (updater: MainView | ((view: MainView) => MainView)) => void;
  clearBrowse: () => void;
  onAndroidDetected: () => void;
}) {
  const [showFolderSetup, setShowFolderSetup] = useState(false);

  // Mobile-only fullscreen "Now Playing" page (replaces the desktop lyrics
  // sidebar on responsive/mobile layouts) and its lyrics/menu sub-views.
  const [mobilePlayerOpen, setMobilePlayerOpen] = useState(false);
  const [mobilePlayerClosing, setMobilePlayerClosing] = useState(false);
  const [mobilePlayerKey, setMobilePlayerKey] = useState(0);
  const mobilePlayerOpenRef = useRef(false);
  const mobilePlayerClosingRef = useRef(false);
  const mobilePlayerCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [mobilePlayerView, setMobilePlayerView] = useState<MobileNowPlayingView>("cover");
  const [mobilePlayerMenuOpen, setMobilePlayerMenuOpen] = useState(false);

  // Settings overlay on narrow layouts; desktop uses mainView === "settings".
  const [mobileSettingsOpen, setMobileSettingsOpen] = useState(false);
  const [mobileSettingsClosing, setMobileSettingsClosing] = useState(false);
  const mobileSettingsClosingRef = useRef(false);
  const mobileSettingsCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const forceCloseMobileSettings = () => {
    if (mobileSettingsCloseTimer.current) {
      clearTimeout(mobileSettingsCloseTimer.current);
      mobileSettingsCloseTimer.current = null;
    }
    mobileSettingsClosingRef.current = false;
    setMobileSettingsClosing(false);
    setMobileSettingsOpen(false);
  };

  const forceCloseMobilePlayer = () => {
    if (mobilePlayerCloseTimer.current) {
      clearTimeout(mobilePlayerCloseTimer.current);
      mobilePlayerCloseTimer.current = null;
    }
    mobilePlayerClosingRef.current = false;
    mobilePlayerOpenRef.current = false;
    setMobilePlayerClosing(false);
    setMobilePlayerOpen(false);
    setMobilePlayerView("cover");
    setMobilePlayerMenuOpen(false);
  };

  const handleCloseMobilePlayer = () => {
    if (!mobilePlayerOpenRef.current || mobilePlayerClosingRef.current) return;
    mobilePlayerClosingRef.current = true;
    // Treat as closed for bar taps immediately so the first real tap after
    // dismiss can reopen (don't wait for the 360ms unmount).
    mobilePlayerOpenRef.current = false;
    setMobilePlayerMenuOpen(false);
    setMobilePlayerClosing(true);
    mobilePlayerCloseTimer.current = setTimeout(() => {
      mobilePlayerClosingRef.current = false;
      setMobilePlayerClosing(false);
      setMobilePlayerOpen(false);
      setMobilePlayerView("cover");
      mobilePlayerCloseTimer.current = null;
    }, 360);
  };

  const handleDragCloseMobilePlayer = () => {
    // Ghost-click guard is armed inside useDragDismiss on dismiss.
    handleCloseMobilePlayer();
  };

  useEffect(() => {
    const media = window.matchMedia("(max-width: 900px)");
    const onChange = () => {
      if (!media.matches) {
        setMobileNavOpen(false);
        // Now Playing is mobile-only; Settings moves into the middle pane on desktop.
        if (mobilePlayerCloseTimer.current) {
          clearTimeout(mobilePlayerCloseTimer.current);
          mobilePlayerCloseTimer.current = null;
        }
        mobilePlayerClosingRef.current = false;
        setMobilePlayerClosing(false);
        setMobilePlayerOpen(false);
        setMobilePlayerView("cover");
        setMobilePlayerMenuOpen(false);
        setMobileSettingsOpen((wasOpen) => {
          if (wasOpen || mobileSettingsClosingRef.current) {
            forceCloseMobileSettings();
            clearBrowse();
            setMainView("settings");
          }
          return false;
        });
      } else {
        setMainView((view) => {
          if (view === "settings") {
            if (mobileSettingsCloseTimer.current) {
              clearTimeout(mobileSettingsCloseTimer.current);
              mobileSettingsCloseTimer.current = null;
            }
            mobileSettingsClosingRef.current = false;
            setMobileSettingsClosing(false);
            setMobileSettingsOpen(true);
            return "home";
          }
          return view;
        });
      }
    };
    onChange();
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!mobileNavOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMobileNavOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mobileNavOpen]);

  useEffect(() => {
    void isAndroid().then((android) => {
      setAndroidHost(android);
      androidHostRef.current = android;
      if (android) {
        // WebView often reports hover:hover, so media-query hover resets never
        // fire — force the no-hover class from the trusted host OS signal.
        enableNoHoverMode();
        onAndroidDetected();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // On Android, prompt for a music folder if Library isn't synced yet
  // and the user hasn't dismissed the welcome prompt.
  useEffect(() => {
    if (!androidHost || playlists.length === 0) return;
    const allLocal = playlists.find((p) => isLibraryPlaylistName(p.name));
    if (allLocal && !allLocal.sync_folder) {
      void Promise.all([listMediaFolders(), isFolderSetupDismissed()])
        .then(([folders, dismissed]) => {
          if (folders.length === 0 && !dismissed) setShowFolderSetup(true);
        })
        .catch(() => setShowFolderSetup(true));
    }
  }, [androidHost, playlists]);

  const skipFolderSetup = async () => {
    setShowFolderSetup(false);
    try {
      await dismissFolderSetup();
    } catch {
      /* ignore */
    }
  };

  return {
    showFolderSetup,
    setShowFolderSetup,
    skipFolderSetup,
    mobilePlayerOpen,
    setMobilePlayerOpen,
    mobilePlayerClosing,
    setMobilePlayerClosing,
    mobilePlayerKey,
    setMobilePlayerKey,
    mobilePlayerOpenRef,
    mobilePlayerClosingRef,
    mobilePlayerCloseTimer,
    mobilePlayerView,
    setMobilePlayerView,
    mobilePlayerMenuOpen,
    setMobilePlayerMenuOpen,
    mobileSettingsOpen,
    setMobileSettingsOpen,
    mobileSettingsClosing,
    setMobileSettingsClosing,
    mobileSettingsClosingRef,
    mobileSettingsCloseTimer,
    forceCloseMobileSettings,
    forceCloseMobilePlayer,
    handleCloseMobilePlayer,
    handleDragCloseMobilePlayer,
  };
}
