export default function DeletePlaylistDialog({
  name,
  onCancel,
  onConfirm,
}: {
  name: string;
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
          <h2>Delete playlist?</h2>
        </div>
        <p className="confirm-text">
          This will permanently delete "{name}". This action cannot be
          undone.
        </p>
        <div className="modal-actions">
          <button className="btn-ghost" onClick={onCancel} type="button">
            Cancel
          </button>
          <button className="btn-danger" onClick={onConfirm} type="button">
            Delete
          </button>
        </div>
      </div>
    </div>
  );
}
