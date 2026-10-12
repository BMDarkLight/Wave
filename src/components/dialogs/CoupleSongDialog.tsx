/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import { BiX } from "react-icons/bi";
import { searchLibraryTracks, type Track } from "../../utils/player";

const titleOf = (track: Track) => track.title || track.name;

/** Pick the song to couple with `track`, and which of the two plays first. */
export default function CoupleSongDialog({
  track,
  onClose,
  onCouple,
}: {
  track: Track;
  onClose: () => void;
  onCouple: (first: string, second: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<Track[]>([]);
  const [loading, setLoading] = useState(true);
  const [partner, setPartner] = useState<Track | null>(null);
  const [trackFirst, setTrackFirst] = useState(true);

  useEffect(() => {
    let current = true;
    setLoading(true);
    const timer = setTimeout(() => {
      searchLibraryTracks(query, 80)
        .then((tracks) => {
          if (current) setResults(tracks.filter((t) => t.path !== track.path));
        })
        .catch(() => {
          if (current) setResults([]);
        })
        .finally(() => {
          if (current) setLoading(false);
        });
    }, 200);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [query, track.path]);

  const couple = () => {
    if (!partner) return;
    if (trackFirst) onCouple(track.path, partner.path);
    else onCouple(partner.path, track.path);
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog library-picker-dialog couple-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>{`Couple "${titleOf(track)}"`}</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>
        <p className="couple-dialog-hint">
          Coupled songs always play back to back, with shuffle on or off.
        </p>
        <label className="modal-label" htmlFor="couple-search-input">
          Couple with
        </label>
        <input
          id="couple-search-input"
          className="modal-input"
          type="search"
          autoFocus
          placeholder="Title, artist, or album"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <div className="library-picker-results" role="listbox">
          {loading && results.length === 0 ? (
            <p className="library-picker-empty">Searching…</p>
          ) : results.length === 0 ? (
            <p className="library-picker-empty">No matching tracks.</p>
          ) : (
            results.map((result) => {
              const isSelected = partner?.path === result.path;
              return (
                <button
                  key={result.path}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  className={`library-picker-row${isSelected ? " selected" : ""}`}
                  onClick={() => setPartner(result)}
                >
                  <span className="library-picker-check" aria-hidden>
                    {isSelected ? "✓" : ""}
                  </span>
                  <span className="library-picker-meta">
                    <span className="library-picker-title">
                      {titleOf(result)}
                    </span>
                    <span className="library-picker-sub">
                      {[result.artist, result.album]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>
        <div className="couple-order" role="radiogroup" aria-label="Order">
          <button
            type="button"
            role="radio"
            aria-checked={trackFirst}
            className={trackFirst ? "active" : ""}
            onClick={() => setTrackFirst(true)}
          >
            {titleOf(track)} first
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={!trackFirst}
            className={trackFirst ? "" : "active"}
            onClick={() => setTrackFirst(false)}
          >
            {partner ? titleOf(partner) : "Picked song"} first
          </button>
        </div>
        <div className="modal-actions">
          <button className="btn-ghost" type="button" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn-primary"
            type="button"
            onClick={couple}
            disabled={!partner}
          >
            Couple
          </button>
        </div>
      </div>
    </div>
  );
}
