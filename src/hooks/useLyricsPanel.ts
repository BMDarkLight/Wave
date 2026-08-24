import { useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { fetchLyricsForTrack, getTrackDetails, getTrackFullCover, type QueueTrackState, type Track } from "../utils/player";
import { parseTimedLyrics } from "../utils/lyrics";
import { useLyricsAutoScroll } from "./useLyricsAutoScroll";

/** Lyrics panel state: which track's lyrics are showing, the auto-fetch of
 * missing lyrics on track change, and the timed (LRC) line-highlighting
 * derived from playback position. */
export function useLyricsPanel({
  currentTrack,
  autoLyricsDownload,
  playbackCurrentPath,
  displayPosition,
  setPlaylist,
  setQueueData,
}: {
  currentTrack: Track | null;
  autoLyricsDownload: boolean;
  playbackCurrentPath: string | null;
  displayPosition: number;
  setPlaylist: Dispatch<SetStateAction<Track[]>>;
  setQueueData: Dispatch<SetStateAction<QueueTrackState>>;
}) {
  const [lyricsPanelTrack, setLyricsPanelTrack] = useState<Track | null>(null);
  const [lyricsFullCover, setLyricsFullCover] = useState<string | null>(null);
  const activeLyricLineRef = useRef<HTMLButtonElement>(null);
  const [lyricsFetchPath, setLyricsFetchPath] = useState<string | null>(null);
  const lyricsFetchIdRef = useRef(0);

  const applyLyricsToTrack = (
    path: string,
    lyrics: string,
    lyricsSource: string | null | undefined,
  ) => {
    setPlaylist((prev) =>
      prev.map((t) =>
        t.path === path
          ? { ...t, lyrics, lyrics_source: lyricsSource ?? t.lyrics_source }
          : t,
      ),
    );
    setQueueData((prev) => ({
      ...prev,
      tracks: prev.tracks.map((t) =>
        t.path === path
          ? { ...t, lyrics, lyrics_source: lyricsSource ?? t.lyrics_source }
          : t,
      ),
    }));
    setLyricsPanelTrack((prev) =>
      prev && prev.path === path
        ? { ...prev, lyrics, lyrics_source: lyricsSource ?? prev.lyrics_source }
        : prev,
    );
  };

  // Load full cover + lyrics details only while the lyrics panel is open.
  useEffect(() => {
    if (!lyricsPanelTrack?.path) {
      setLyricsFullCover(null);
      return;
    }
    let cancelled = false;
    const path = lyricsPanelTrack.path;
    void (async () => {
      const [fullCover, details] = await Promise.all([
        getTrackFullCover(path),
        getTrackDetails(path),
      ]);
      if (cancelled) return;
      if (fullCover) setLyricsFullCover(fullCover);
      else setLyricsFullCover(null);
      if (details?.lyrics) {
        applyLyricsToTrack(path, details.lyrics, details.lyrics_source);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lyricsPanelTrack?.path]);

  // Close lyrics panel and auto-fetch lyrics when track changes
  useEffect(() => {
    if (!currentTrack) {
      setLyricsFetchPath(null);
      return;
    }
    setLyricsPanelTrack(null);
    if (
      currentTrack.lyrics &&
      (parseTimedLyrics(currentTrack.lyrics) || currentTrack.lyrics_source === "lrclib")
    ) {
      setLyricsFetchPath(null);
      return;
    }
    if (!autoLyricsDownload) {
      setLyricsFetchPath(null);
      return;
    }

    const path = currentTrack.path;
    const fetchId = ++lyricsFetchIdRef.current;
    setLyricsFetchPath(path);

    let cancelled = false;
    fetchLyricsForTrack(path)
      .then((updated) => {
        if (cancelled || lyricsFetchIdRef.current !== fetchId) return;
        setLyricsFetchPath(null);
        if (!updated?.lyrics) return;
        applyLyricsToTrack(path, updated.lyrics, updated.lyrics_source);
      })
      .catch(() => {
        if (!cancelled && lyricsFetchIdRef.current === fetchId) {
          setLyricsFetchPath(null);
        }
      });

    return () => {
      cancelled = true;
      if (lyricsFetchIdRef.current === fetchId) {
        lyricsFetchIdRef.current += 1;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentTrack?.path, autoLyricsDownload]);

  const cancelLyricsFetch = () => {
    lyricsFetchIdRef.current += 1;
    setLyricsFetchPath(null);
  };

  // Live (LRC-style) timestamped lyrics for the open lyrics panel.
  const timedLyrics = useMemo(
    () => parseTimedLyrics(lyricsPanelTrack?.lyrics),
    [lyricsPanelTrack?.lyrics],
  );
  const isLyricsPanelOnCurrentTrack =
    !!lyricsPanelTrack && lyricsPanelTrack.path === playbackCurrentPath;
  const activeLyricIndex = useMemo(() => {
    if (!timedLyrics || !isLyricsPanelOnCurrentTrack) return -1;
    let idx = -1;
    for (let i = 0; i < timedLyrics.length; i++) {
      if (timedLyrics[i].time > displayPosition) break;
      idx = i;
    }
    return idx;
  }, [timedLyrics, isLyricsPanelOnCurrentTrack, displayPosition]);

  const lyricsScrollHandlers = useLyricsAutoScroll(
    activeLyricIndex,
    !!lyricsPanelTrack,
    activeLyricLineRef,
  );

  return {
    lyricsPanelTrack,
    setLyricsPanelTrack,
    lyricsFullCover,
    activeLyricLineRef,
    lyricsFetchPath,
    cancelLyricsFetch,
    applyLyricsToTrack,
    timedLyrics,
    isLyricsPanelOnCurrentTrack,
    activeLyricIndex,
    lyricsScrollHandlers,
  };
}
