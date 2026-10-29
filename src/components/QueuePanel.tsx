/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useContext } from "react";
import { BiDotsHorizontalRounded, BiX } from "react-icons/bi";
import Artwork from "./Artwork";
import VirtualizedList from "./VirtualizedList";
import type { ContextMenuAnchor } from "./ContextMenu";
import { formatTime } from "../utils/format";
import { getTrackTitle } from "../utils/track";
import type { Track } from "../utils/player";
import { RemoteStatusContext, unavailableReason } from "../utils/remote";
import RemoteBadge from "./RemoteBadge";

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
  const remoteStatus = useContext(RemoteStatusContext);
  return (
    <div className="right-panel-content">
      <div className="right-panel-header">
        <h2>
          Queue
          {tracks.length > 0 && (
            <span className="right-panel-count">
              {tracks.length === 1 ? "1 song" : `${tracks.length} songs`}
            </span>
          )}
        </h2>
        <div className="right-panel-header-actions">
          {tracks.length > 0 && (
            <button
              className="right-panel-text-btn"
              onClick={onClear}
              type="button"
            >
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
              const unavailable = unavailableReason(track, remoteStatus);
              return (
                <div
                  className={`queue-item ${currentIndex === index ? "active" : ""} ${currentIndex !== null && index < currentIndex ? "played" : ""} ${menuIndex === index ? "menu-open" : ""} ${unavailable ? "track-unavailable" : ""}`}
                  title={unavailable ?? undefined}
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
                    <div className="queue-item-name">
                      {getTrackTitle(track)}
                      <RemoteBadge track={track} />
                    </div>
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
