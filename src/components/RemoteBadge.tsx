/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { BiCloud, BiSolidCloud } from "react-icons/bi";
import type { Track } from "../utils/player";
import { isRemote } from "../utils/remote";

/** Marks a track that lives on a server. Filled once it is fully cached. */
export default function RemoteBadge({ track }: { track: Track }) {
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
