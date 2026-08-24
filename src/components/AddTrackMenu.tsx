import { createPortal } from "react-dom";
import { BiFolderOpen, BiPlus } from "react-icons/bi";

export default function AddTrackMenu({
  anchor,
  onClose,
  androidHost,
  onAddFiles,
  onAddFolder,
  onAddFolderAsPlaylist,
}: {
  anchor: { top: number; left: number };
  onClose: () => void;
  androidHost: boolean;
  onAddFiles: () => void;
  onAddFolder: () => void;
  onAddFolderAsPlaylist: () => void;
}) {
  return createPortal(
    <>
      <div className="context-menu-backdrop" onClick={onClose} />
      <div
        className="add-track-menu"
        style={{
          position: "fixed",
          top: `${anchor.top}px`,
          left: `${anchor.left}px`,
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <button type="button" onClick={onAddFiles}>
          <BiPlus /> Add files
        </button>
        {!androidHost && (
          <>
            <button type="button" onClick={onAddFolder}>
              <BiFolderOpen /> Add folder
            </button>
            <button type="button" onClick={onAddFolderAsPlaylist}>
              <BiFolderOpen /> Add folder as playlist
            </button>
          </>
        )}
        {androidHost && (
          <p className="add-track-menu-hint">
            On Android, tap the + button to scan a music folder into Library.
          </p>
        )}
      </div>
    </>,
    document.body,
  );
}
