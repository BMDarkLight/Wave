/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// The Search tab on the narrow layout: a page with the search field at the
// top and the same results the desktop search shows beneath it.

import type { ReactNode, RefObject } from "react";
import { BiSearch, BiX } from "react-icons/bi";

interface MobileSearchPageProps {
  inputRef: RefObject<HTMLInputElement | null>;
  query: string;
  onQueryChange: (query: string) => void;
  results: ReactNode;
}

export default function MobileSearchPage({
  inputRef,
  query,
  onQueryChange,
  results,
}: MobileSearchPageProps) {
  return (
    <main className="main-content mobile-search-page">
      <div className="msearch-header">
        <h1>Search</h1>
        <div className="msearch-field">
          <BiSearch className="library-search-icon" aria-hidden />
          <input
            ref={inputRef}
            className="library-search-input"
            type="search"
            placeholder="Songs, artists, albums, lyrics"
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            aria-label="Search library"
            autoComplete="off"
            spellCheck={false}
          />
          {query && (
            <button
              className="library-search-clear"
              type="button"
              onClick={() => {
                onQueryChange("");
                inputRef.current?.focus();
              }}
              title="Clear search"
              aria-label="Clear search"
            >
              <BiX />
            </button>
          )}
        </div>
      </div>
      {query.trim() ? (
        results
      ) : (
        <p className="msearch-hint">
          Find anything in your library, even a line from the lyrics.
        </p>
      )}
    </main>
  );
}
