import type { RefObject } from "react";
import { BiFolderOpen, BiSync, BiX } from "react-icons/bi";
import { getFileName } from "../../utils/player";

export type PlaylistDialogState =
  | { mode: "create" }
  | { mode: "rename"; playlistId: string; currentName: string };

export default function CreatePlaylistDialog({
  dialog,
  onClose,
  onSubmit,
  nameInputRef,
  name,
  onNameChange,
  androidHost,
  syncFolder,
  onClearSyncFolder,
  onPickSyncFolder,
  error,
}: {
  dialog: PlaylistDialogState;
  onClose: () => void;
  onSubmit: () => void;
  nameInputRef: RefObject<HTMLInputElement | null>;
  name: string;
  onNameChange: (value: string) => void;
  androidHost: boolean;
  syncFolder: string | null;
  onClearSyncFolder: () => void;
  onPickSyncFolder: () => void;
  error: string | null;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog playlist-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>{dialog.mode === "create" ? "Create playlist" : "Rename playlist"}</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            onSubmit();
          }}
        >
          <label className="modal-label" htmlFor="playlist-name-input">
            Name
          </label>
          <input
            id="playlist-name-input"
            ref={nameInputRef}
            className="modal-input"
            type="text"
            value={name}
            onChange={(event) => onNameChange(event.target.value)}
            placeholder="My playlist"
            autoComplete="off"
          />
          {!androidHost && dialog.mode === "create" && (
            <div className="playlist-sync-field">
              <div className="playlist-sync-inline">
                <span className="modal-label">Sync with folder</span>
                <span className="modal-hint inline-hint">
                  Optional. Keep this playlist tied to a music folder.
                </span>
                {syncFolder ? (
                  <div className="playlist-sync-selected">
                    <BiSync className="playlist-sync-icon" />
                    <span className="playlist-sync-path" title={syncFolder}>
                      {getFileName(syncFolder)}
                    </span>
                    <button
                      type="button"
                      className="btn-ghost btn-sm"
                      onClick={onClearSyncFolder}
                    >
                      Clear
                    </button>
                  </div>
                ) : (
                  <button
                    type="button"
                    className="playlist-sync-pick"
                    onClick={onPickSyncFolder}
                  >
                    <BiFolderOpen /> Choose
                  </button>
                )}
              </div>
            </div>
          )}
          {error && <p className="modal-error">{error}</p>}
          <div className="modal-actions">
            <button className="btn-ghost" onClick={onClose} type="button">
              Cancel
            </button>
            <button className="btn-primary" type="submit">
              {dialog.mode === "create" ? "Create" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
