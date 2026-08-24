import { BiX } from "react-icons/bi";

export default function StatusToasts({
  showExitToast,
  crashReport,
  onDismissCrashReport,
  error,
  onDismissError,
  lyricsFetchPath,
  onCancelLyricsFetch,
  isLoading,
}: {
  showExitToast: boolean;
  crashReport: string | null;
  onDismissCrashReport: () => void;
  error: string | null;
  onDismissError: () => void;
  lyricsFetchPath: string | null;
  onCancelLyricsFetch: () => void;
  isLoading: boolean;
}) {
  return (
    <>
      {showExitToast && (
        <div className="exit-toast" role="status" aria-live="polite">
          Press back again to close Wave
        </div>
      )}

      {crashReport && (
        <div
          className="crash-report-overlay"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="crash-report-title"
        >
          <div className="crash-report-card">
            <h2 id="crash-report-title">Wave recovered from a crash</h2>
            <p>
              A previous launch failed. Copy this report when filing a bug —
              no adb needed. Dismiss once you have copied it.
            </p>
            <pre className="crash-report-body">{crashReport}</pre>
            <div className="crash-report-actions">
              <button
                type="button"
                onClick={() => {
                  void navigator.clipboard
                    ?.writeText(crashReport)
                    .catch(() => {});
                }}
              >
                Copy
              </button>
              <button type="button" onClick={onDismissCrashReport}>
                Dismiss
              </button>
            </div>
          </div>
        </div>
      )}

      {error && (
        <div className="error-toast" role="alert" aria-live="assertive">
          {error}
          <button onClick={onDismissError} type="button">
            <BiX />
          </button>
        </div>
      )}

      {lyricsFetchPath && (
        <div
          className="loading-indicator lyrics-fetch-indicator"
          role="status"
          aria-live="polite"
        >
          <div className="spinner" /> Fetching
          <button
            className="loading-cancel-btn"
            onClick={onCancelLyricsFetch}
            type="button"
          >
            Cancel
          </button>
        </div>
      )}

      {isLoading && (
        <div className="loading-indicator" role="status" aria-live="polite">
          <div className="spinner" /> Loading...
        </div>
      )}
    </>
  );
}
