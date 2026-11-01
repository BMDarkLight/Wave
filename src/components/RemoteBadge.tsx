/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiCloud, BiGlobe, BiSolidCloud } from "react-icons/bi";
import type { Track } from "../utils/player";
import { isOnlineSong, isRemote } from "../utils/remote";

/** Marks a track that lives on a server, filled once it is fully cached, and
 * a song streamed from an online catalog with an "Online" label. */
export default function RemoteBadge({ track }: { track: Track }) {
  if (isOnlineSong(track)) return <OnlineLabel />;
  if (!isRemote(track)) return null;
  const cached = track.cache_state === "complete";
  return (
    <span
      className="track-remote-badge"
      title={cached ? "On your server, saved for offline" : "On your server"}
      aria-label={cached ? "Remote, cached" : "Remote"}
    >
      {cached ? <BiSolidCloud /> : <BiCloud />}
    </span>
  );
}

/** Says a song is streamed online, not kept in the library. */
export function OnlineLabel() {
  return (
    <span
      className="track-online-label"
      title="Streamed online, not in your library"
    >
      <BiGlobe aria-hidden /> Online
    </span>
  );
}
