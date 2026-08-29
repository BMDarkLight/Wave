import { BiCloudDownload, BiCheck, BiErrorCircle } from "react-icons/bi";
import type { ProviderResults, SourceTrack } from "../utils/player";
import { sourceTrackKey } from "../hooks/useSourceSearch";
import { highlightMatch } from "../utils/search";
import { formatTime } from "../utils/format";

type Props = {
  results: ProviderResults[];
  loading: boolean;
  searched: boolean;
  error: string | null;
  /** Keys of results with a stream or download in flight. */
  busy: Record<string, "stream" | "download">;
  query: string;
  /** Play a result — streaming it first if it isn't already owned. */
  onPlay: (track: SourceTrack) => void;
  /** Keep a result in the library. */
  onDownload: (track: SourceTrack) => void;
};

/**
 * Tier 3 results, grouped by provider.
 *
 * A provider that errored renders as its own unavailable section rather than
 * disappearing, so a Jamendo outage never looks like "no results anywhere".
 */
export function SourceResults({
  results,
  loading,
  searched,
  error,
  busy,
  query,
  onPlay,
  onDownload,
}: Props) {
  if (loading) {
    return (
      <div className="source-results">
        <div className="source-section-head">
          <span className="import-spinner" />
          <span>Searching sources…</span>
        </div>
      </div>
    );
  }

  if (!searched) return null;

  if (error) {
    return (
      <div className="source-results">
        <div className="source-section-error">
          <BiErrorCircle />
          <span>{error}</span>
        </div>
      </div>
    );
  }

  const totalHits = results.reduce((sum, r) => sum + r.tracks.length, 0);
  if (totalHits === 0 && results.every((r) => !r.error)) {
    return (
      <div className="source-results">
        <div className="source-section-empty">
          No sources had anything matching “{query.trim()}”.
        </div>
      </div>
    );
  }

  return (
    <div className="source-results">
      {results.map((section) => (
        <section key={section.provider} className="source-section">
          <div className="source-section-head">
            <h3>{section.display_name}</h3>
            {section.error ? null : (
              <span className="source-section-count">
                {section.tracks.length}
              </span>
            )}
          </div>

          {section.error ? (
            <div className="source-section-error">
              <BiErrorCircle />
              <span>{section.error}</span>
            </div>
          ) : section.tracks.length === 0 ? (
            <div className="source-section-empty">No matches here.</div>
          ) : (
            <div className="source-hit-list">
              {section.tracks.map((track) => {
                const key = sourceTrackKey(track);
                const state = busy[key];
                const owned = !!track.already_in_library;
                return (
                  <div
                    key={key}
                    className={`source-hit ${state ? "busy" : ""}`}
                  >
                    <button
                      type="button"
                      className="source-hit-main"
                      disabled={!!state}
                      onClick={() => onPlay(track)}
                      title={
                        owned
                          ? "Already in your library — plays your copy"
                          : track.is_full_length
                            ? "Stream this track"
                            : "Play the 30-second preview"
                      }
                    >
                      {track.artwork_url ? (
                        <img
                          className="track-thumb source-hit-thumb"
                          src={track.artwork_url}
                          alt=""
                          loading="lazy"
                        />
                      ) : (
                        <div className="track-thumb source-hit-thumb source-hit-thumb-empty">
                          {track.title.slice(0, 1).toUpperCase()}
                        </div>
                      )}

                      <div className="source-hit-body">
                        <div className="source-hit-title">
                          {highlightMatch(track.title, query)}
                        </div>
                        <div className="source-hit-meta">
                          {highlightMatch(track.artist, query)}
                          {track.album ? (
                            <>
                              {" · "}
                              {highlightMatch(track.album, query)}
                            </>
                          ) : null}
                        </div>
                        <div className="source-hit-chips">
                          {!track.is_full_length && (
                            <span className="source-chip source-chip-preview">
                              30s preview
                            </span>
                          )}
                          {owned && (
                            <span className="source-chip source-chip-owned">
                              <BiCheck /> In your library
                            </span>
                          )}
                          {track.attribution && (
                            <span className="source-chip source-chip-licence">
                              {track.attribution}
                            </span>
                          )}
                        </div>
                      </div>

                      <div className="source-hit-duration">
                        {state === "stream"
                          ? "…"
                          : formatTime(track.duration_seconds)}
                      </div>
                    </button>

                    {track.downloadable && !owned && (
                      <button
                        type="button"
                        className="source-hit-download"
                        disabled={!!state}
                        onClick={() => onDownload(track)}
                        title="Save to your library"
                        aria-label={`Save ${track.title} to your library`}
                      >
                        {state === "download" ? (
                          <span className="import-spinner" />
                        ) : (
                          <BiCloudDownload />
                        )}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>
      ))}
    </div>
  );
}
