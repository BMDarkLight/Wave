import { useEffect, useRef, useState } from "react";
import { searchLibrary, type SearchHit } from "../utils/player";

/** Main library search box state: query, debounced results fetch, and
 * open/close of the search overlay (desktop input + mobile topbar). */
export function useLibrarySearch() {
  const [mainSearchQuery, setMainSearchQuery] = useState("");
  const [mainSearchHits, setMainSearchHits] = useState<SearchHit[]>([]);
  const [mainSearchLoading, setMainSearchLoading] = useState(false);
  const [mainSearchFullLibrary, setMainSearchFullLibrary] = useState(false);
  const [mainSearchOpen, setMainSearchOpen] = useState(false);
  const mainSearchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mainSearchReqId = useRef(0);
  const mainSearchInputRef = useRef<HTMLInputElement | null>(null);
  const mobileSearchInputRef = useRef<HTMLInputElement | null>(null);

  const focusMainSearchInput = () => {
    const mobile =
      typeof window !== "undefined" &&
      window.matchMedia("(max-width: 900px)").matches;
    const input = mobile
      ? mobileSearchInputRef.current
      : mainSearchInputRef.current;
    input?.focus();
    input?.select();
  };

  const openMainSearch = () => {
    setMainSearchOpen(true);
  };
  const closeMainSearch = () => {
    setMainSearchOpen(false);
    setMainSearchQuery("");
    setMainSearchHits([]);
    setMainSearchFullLibrary(false);
  };
  const toggleMainSearch = () => {
    if (mainSearchOpen) closeMainSearch();
    else openMainSearch();
  };

  // Realtime main search — short debounce so typing stays tactile.
  useEffect(() => {
    const q = mainSearchQuery.trim();
    if (!q) {
      setMainSearchHits([]);
      setMainSearchLoading(false);
      return;
    }
    if (mainSearchTimer.current) clearTimeout(mainSearchTimer.current);
    setMainSearchLoading(true);
    const reqId = ++mainSearchReqId.current;
    mainSearchTimer.current = setTimeout(() => {
      searchLibrary(q, 100)
        .then((hits) => {
          if (mainSearchReqId.current !== reqId) return;
          setMainSearchHits(hits);
        })
        .catch(() => {
          if (mainSearchReqId.current !== reqId) return;
          setMainSearchHits([]);
        })
        .finally(() => {
          if (mainSearchReqId.current === reqId) setMainSearchLoading(false);
        });
    }, 80);
    return () => {
      if (mainSearchTimer.current) {
        clearTimeout(mainSearchTimer.current);
        mainSearchTimer.current = null;
      }
    };
  }, [mainSearchQuery]);

  return {
    mainSearchQuery,
    setMainSearchQuery,
    mainSearchHits,
    mainSearchLoading,
    mainSearchFullLibrary,
    setMainSearchFullLibrary,
    mainSearchOpen,
    mainSearchInputRef,
    mobileSearchInputRef,
    focusMainSearchInput,
    openMainSearch,
    closeMainSearch,
    toggleMainSearch,
  };
}
