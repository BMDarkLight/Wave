import type { PlaylistInfo } from "../../utils/player";

export default function AddToPlaylistDialog({
  playlists,
  excludePlaylistId,
  onClose,
  onSelect,
}: {
  playlists: PlaylistInfo[];
  excludePlaylistId: string | null;
  onClose: () => void;
  onSelect: (playlistId: string) => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>Add to playlist</h2>
          <button className="modal-close-btn" onClick={onClose} type="button">
            ×
          </button>
        </div>
        <div className="playlist-picker-list">
          {playlists
            .filter(
              (p) => p.id !== excludePlaylistId && p.name !== "Favorites",
            )
            .map((p) => (
              <button
                key={p.id}
                className="playlist-picker-item"
                type="button"
                onClick={() => onSelect(p.id)}
              >
                {p.name}
              </button>
            ))}
        </div>
      </div>
    </div>
  );
}
