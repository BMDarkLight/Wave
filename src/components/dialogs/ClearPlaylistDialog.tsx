export default function ClearPlaylistDialog({
  onCancel,
  onConfirm,
}: {
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal-dialog confirm-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onCancel();
        }}
      >
        <div className="modal-header">
          <h2>Clear playlist?</h2>
        </div>
        <p className="confirm-text">
          This will remove all tracks from this playlist. The files on disk
          won't be affected.
        </p>
        <div className="modal-actions">
          <button className="btn-ghost" onClick={onCancel} type="button">
            Cancel
          </button>
          <button className="btn-primary" onClick={onConfirm} type="button">
            Clear
          </button>
        </div>
      </div>
    </div>
  );
}
