import { useEffect, useRef, useState } from "react";
import {
  searchLibrary,
  searchLibraryCollections,
  type AlbumSummary,
  type ArtistSummary,
  type SearchHit,
} from "../utils/player";

const NO_COLLECTIONS: { albums: AlbumSummary[]; artists: ArtistSummary[] } = {
  albums: [],
  artists: [],
};

/** Main library search box state: query, debounced results fetch, and
 * open/close of the search overlay (desktop input + mobile topbar). */
export function useLibrarySearch() {
  const [mainSearchQuery, setMainSearchQuery] = useState("");
  const [mainSearchHits, setMainSearchHits] = useState<SearchHit[]>([]);
  const [mainSearchAlbums, setMainSearchAlbums] = useState<AlbumSummary[]>([]);
  const [mainSearchArtists, setMainSearchArtists] = useState<ArtistSummary[]>(
    [],
  );
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
    setMainSearchAlbums([]);
    setMainSearchArtists([]);
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
      // Invalidate any request still in flight. Without this, a response for
      // the query the user just cleared still counts as current and refills
      // the list underneath an empty box.
      mainSearchReqId.current += 1;
      setMainSearchHits([]);
      setMainSearchAlbums([]);
      setMainSearchArtists([]);
      setMainSearchLoading(false);
      return;
    }
    if (mainSearchTimer.current) clearTimeout(mainSearchTimer.current);
    setMainSearchLoading(true);
    const reqId = ++mainSearchReqId.current;
    mainSearchTimer.current = setTimeout(() => {
      // Tracks and collections share one debounce and one request id, so the
      // three lists can never come from different queries.
      Promise.all([
        searchLibrary(q, 100).catch(() => [] as SearchHit[]),
        searchLibraryCollections(q, 12).catch(() => NO_COLLECTIONS),
      ])
        .then(([hits, collections]) => {
          if (mainSearchReqId.current !== reqId) return;
          setMainSearchHits(hits);
          setMainSearchAlbums(collections.albums);
          setMainSearchArtists(collections.artists);
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
    mainSearchAlbums,
    mainSearchArtists,
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
