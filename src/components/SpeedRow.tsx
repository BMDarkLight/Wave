/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { MAX_SPEED, MIN_SPEED, formatSpeed } from "../utils/player";

/** Speed slider row, laid out like the crossfade row it sits under. */
export default function SpeedRow({
  id,
  speed,
  onChange,
}: {
  id: string;
  speed: number;
  onChange: (speed: number) => void;
}) {
  return (
    <div
      className="eq-crossfade eq-subrow"
      onPointerDown={(event) => event.stopPropagation()}
      onClick={(event) => event.stopPropagation()}
    >
      <label className="eq-crossfade-label" htmlFor={id}>
        Speed
      </label>
      <input
        id={id}
        type="range"
        min={MIN_SPEED}
        max={MAX_SPEED}
        step={0.05}
        value={speed}
        onChange={(event) => onChange(Number(event.target.value))}
        aria-label="Playback speed"
      />
      <button
        type="button"
        className="eq-crossfade-value eq-speed-value"
        onDoubleClick={() => onChange(1)}
        title="Double-click for normal speed"
      >
        {formatSpeed(speed)}
      </button>
    </div>
  );
}
