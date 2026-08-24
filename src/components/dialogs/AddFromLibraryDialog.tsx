import type { Track } from "../../utils/player";

export default function AddFromLibraryDialog({
  onClose,
  query,
  onQueryChange,
  loading,
  adding,
  results,
  selected,
  onToggleSelect,
  onPickFile,
  onAddSelected,
}: {
  onClose: () => void;
  query: string;
  onQueryChange: (value: string) => void;
  loading: boolean;
  adding: boolean;
  results: Track[];
  selected: Set<string>;
  onToggleSelect: (path: string) => void;
  onPickFile: () => void;
  onAddSelected: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog library-picker-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>Add from library</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            aria-label="Close"
          >
            ×
          </button>
        </div>
        <label className="modal-label" htmlFor="library-search-input">
          Search
        </label>
        <input
          id="library-search-input"
          className="modal-input"
          type="search"
          autoFocus
          placeholder="Title, artist, or album"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          disabled={adding}
        />
        <div
          className="library-picker-results"
          role="listbox"
          aria-multiselectable="true"
        >
          {loading ? (
            <p className="library-picker-empty">Searching…</p>
          ) : results.length === 0 ? (
            <p className="library-picker-empty">
              {query.trim()
                ? "No matching tracks."
                : "Scan a media folder into Library first, or pick a file below."}
            </p>
          ) : (
            results.map((track) => {
              const isSelected = selected.has(track.path);
              return (
                <button
                  key={track.path}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  className={`library-picker-row${isSelected ? " selected" : ""}`}
                  onClick={() => onToggleSelect(track.path)}
                  disabled={adding}
                >
                  <span className="library-picker-check" aria-hidden>
                    {isSelected ? "✓" : ""}
                  </span>
                  <span className="library-picker-meta">
                    <span className="library-picker-title">
                      {track.title || track.name}
                    </span>
                    <span className="library-picker-sub">
                      {[track.artist, track.album].filter(Boolean).join(" · ")}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>
        <div className="modal-actions library-picker-actions">
          <button
            className="btn-ghost"
            type="button"
            onClick={onPickFile}
            disabled={adding}
          >
            Pick a file…
          </button>
          <div className="library-picker-actions-end">
            <button
              className="btn-ghost"
              type="button"
              onClick={onClose}
              disabled={adding}
            >
              Cancel
            </button>
            <button
              className="btn-primary"
              type="button"
              onClick={onAddSelected}
              disabled={adding || selected.size === 0}
            >
              {adding ? "Adding…" : `Add${selected.size > 0 ? ` (${selected.size})` : ""}`}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
