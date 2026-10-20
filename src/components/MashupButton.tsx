/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useState } from "react";
import { BiLinkAlt } from "react-icons/bi";

/** A reshuffle is instant, so the icon spins at least this long to show it. */
const SPIN_MS = 600;

/** Takes the shuffle button's place while a mashup plays. A press shuffles
 * the mashup again, with the link spinning while it does. */
export default function MashupButton({
  onReshuffle,
}: {
  onReshuffle: () => Promise<void>;
}) {
  const [spinning, setSpinning] = useState(false);

  const reshuffle = async () => {
    if (spinning) return;
    setSpinning(true);
    try {
      await Promise.all([
        onReshuffle(),
        new Promise((resolve) => setTimeout(resolve, SPIN_MS)),
      ]);
    } finally {
      setSpinning(false);
    }
  };

  return (
    <button
      className={`control-btn shuffle-btn mashup-btn active${spinning ? " spinning" : ""}`}
      onClick={() => void reshuffle()}
      type="button"
      title="Playing a mashup. Reshuffle it"
      aria-label="Reshuffle the mashup"
      aria-busy={spinning}
    >
      <BiLinkAlt />
    </button>
  );
}
