import { BiDotsHorizontalRounded, BiX } from "react-icons/bi";
import Artwork from "./Artwork";
import VirtualizedList from "./VirtualizedList";
import type { ContextMenuAnchor } from "./ContextMenu";
import { formatTime } from "../utils/format";
import { getTrackTitle } from "../utils/track";
import type { Track } from "../utils/player";

export default function QueuePanel({
  tracks,
  currentIndex,
  menuIndex,
  onClose,
  onClear,
  onPlayFromQueue,
  onOpenMenu,
  onCloseMenu,
  onRemove,
}: {
  tracks: Track[];
  currentIndex: number | null;
  menuIndex: number | null;
  onClose: () => void;
  onClear: () => void;
  onPlayFromQueue: (index: number) => void;
  onOpenMenu: (index: number, anchor: ContextMenuAnchor) => void;
  onCloseMenu: () => void;
  onRemove: (index: number) => void;
}) {
  return (
    <div className="right-panel-content">
      <div className="right-panel-header">
        <h2>Queue</h2>
        <div className="right-panel-header-actions">
          {tracks.length > 0 && (
            <button className="btn-ghost btn-sm" onClick={onClear} type="button">
              Clear
            </button>
          )}
          <button
            className="right-panel-close"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>
      </div>
      <div className="right-panel-list">
        {tracks.length === 0 ? (
          <div className="queue-empty">
            <p>Queue is empty</p>
            <span>Add tracks with "Play Next" or "Add to Queue"</span>
          </div>
        ) : (
          <VirtualizedList
            count={tracks.length}
            estimateSize={58}
            overscan={12}
            scrollSelector=".right-panel-list"
            className="queue-list-virtual"
          >
            {(index) => {
              const track = tracks[index];
              if (!track) return null;
              return (
                <div
                  className={`queue-item ${currentIndex === index ? "active" : ""} ${menuIndex === index ? "menu-open" : ""}`}
                  onClick={() => onPlayFromQueue(index)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onOpenMenu(index, {
                      top: event.clientY,
                      left: event.clientX,
                      flipAbove: event.clientY,
                    });
                  }}
                >
                  <Artwork
                    track={track}
                    fallback={getTrackTitle(track).slice(0, 1).toUpperCase()}
                    className="queue-thumb"
                  />
                  <div className="queue-item-info">
                    <div className="queue-item-name">{getTrackTitle(track)}</div>
                    <div className="queue-item-artist">{track.artist}</div>
                  </div>
                  <div className="queue-item-duration">
                    {formatTime(track.duration_seconds)}
                  </div>
                  <div className="queue-item-actions">
                    <button
                      className="queue-item-menu"
                      onClick={(event) => {
                        event.stopPropagation();
                        if (menuIndex === index) {
                          onCloseMenu();
                        } else {
                          const rect =
                            event.currentTarget.getBoundingClientRect();
                          onOpenMenu(index, {
                            top: rect.bottom + 4,
                            flipAbove: rect.top - 4,
                            right: window.innerWidth - rect.right,
                          });
                        }
                      }}
                      title="More"
                      type="button"
                    >
                      <BiDotsHorizontalRounded />
                    </button>
                    <button
                      className="queue-item-remove"
                      onClick={(e) => {
                        e.stopPropagation();
                        onRemove(index);
                      }}
                      title="Remove from queue"
                      type="button"
                    >
                      <BiX />
                    </button>
                  </div>
                </div>
              );
            }}
          </VirtualizedList>
        )}
      </div>
    </div>
  );
}
