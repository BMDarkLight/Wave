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
import { BiLink } from "react-icons/bi";
import { SongCouplingContext } from "../utils/songCoupling";

/** A link icon for a coupled song, naming its partner on hover. Renders
 * nothing for a song that is not coupled. */
export default function CoupleMarker({ path }: { path: string }) {
  const couples = useContext(SongCouplingContext);
  const couple = couples.find((c) => c.first === path || c.second === path);
  if (!couple) return null;
  const label =
    couple.first === path
      ? `Plays before ${couple.second_title}`
      : `Plays after ${couple.first_title}`;
  return (
    <span
      className="track-meta-icon-wrap couple-marker"
      title={label}
      aria-label={label}
    >
      <BiLink className="track-meta-icon" aria-hidden />
    </span>
  );
}
