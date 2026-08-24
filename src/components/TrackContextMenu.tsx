import {
  BiAlbum,
  BiListPlus,
  BiListUl,
  BiMinus,
  BiSkipNext,
  BiTrash,
  BiUser,
} from "react-icons/bi";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";
import type { Track } from "../utils/player";

export default function TrackContextMenu({
  track,
  anchor,
  onClose,
  canAddToPlaylist,
  canRemoveFromPlaylist,
  onPlayNext,
  onAddToQueue,
  onAddToPlaylist,
  onGoToAlbum,
  onGoToArtist,
  onRemoveFromPlaylist,
  onRemoveFromLibrary,
}: {
  track: Track;
  anchor: ContextMenuAnchor;
  onClose: () => void;
  canAddToPlaylist: boolean;
  canRemoveFromPlaylist: boolean;
  onPlayNext: (path: string) => void;
  onAddToQueue: (path: string) => void;
  onAddToPlaylist: (path: string) => void;
  onGoToAlbum: (album: string, albumArtist: string | null) => void;
  onGoToArtist: (artist: string) => void;
  onRemoveFromPlaylist: (path: string) => void;
  onRemoveFromLibrary: (path: string) => void;
}) {
  return (
    <ContextMenu anchor={anchor} onClose={onClose}>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
          onPlayNext(track.path);
        }}
      >
        <BiSkipNext /> Play Next
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
          onAddToQueue(track.path);
        }}
      >
        <BiListPlus /> Add to Queue
      </button>
      {canAddToPlaylist && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onAddToPlaylist(track.path);
          }}
        >
          <BiListUl /> Add to Playlist...
        </button>
      )}
      {track.album && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onGoToAlbum(track.album, track.album_artist || track.artist);
          }}
        >
          <BiAlbum /> Go to Album
        </button>
      )}
      {track.artist && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onGoToArtist(track.artist);
          }}
        >
          <BiUser /> Go to Artist
        </button>
      )}
      {canRemoveFromPlaylist && (
        <button
          className="delete-action"
          type="button"
          role="menuitem"
          onClick={() => {
            onClose();
            onRemoveFromPlaylist(track.path);
          }}
        >
          <BiMinus /> Remove from Playlist
        </button>
      )}
      <button
        className="delete-action"
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
          onRemoveFromLibrary(track.path);
        }}
      >
        <BiTrash /> Remove from Library
      </button>
    </ContextMenu>
  );
}
