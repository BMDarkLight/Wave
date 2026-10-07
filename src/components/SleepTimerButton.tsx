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
import { BiCheck, BiMoon, BiX } from "react-icons/bi";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";
import { formatTime } from "../utils/format";
import {
  SLEEP_CHOICES,
  type SleepRequest,
  type SleepTimerStatus,
} from "../utils/player";

/** What the button shows beside the moon while the timer runs. */
function sleepLabel(status: SleepTimerStatus): string | null {
  if (status.mode === "countdown") {
    return formatTime(Math.ceil(status.remaining_seconds ?? 0));
  }
  if (status.mode === "end_of_track") return "End";
  return null;
}

function isCurrent(status: SleepTimerStatus, request: SleepRequest): boolean {
  // A countdown is always partway through, so only end-of-track can match.
  return request.mode === "end_of_track" && status.mode === "end_of_track";
}

export default function SleepTimerButton({
  status,
  onSet,
  className,
}: {
  status: SleepTimerStatus;
  onSet: (request: SleepRequest) => void;
  /** Button styling of the surrounding controls. */
  className: string;
}) {
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);
  const label = sleepLabel(status);
  const active = status.mode !== "off";

  const choose = (request: SleepRequest) => {
    setAnchor(null);
    onSet(request);
  };

  return (
    <>
      <button
        className={`${className} sleep-timer-btn ${active ? "active" : ""}`}
        onClick={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          setAnchor({
            top: rect.bottom + 6,
            right: window.innerWidth - rect.right,
            flipAbove: rect.top - 6,
          });
        }}
        type="button"
        title={
          status.mode === "countdown"
            ? `Sleep timer: pauses in ${label}`
            : status.mode === "end_of_track"
              ? "Sleep timer: pauses when this track ends"
              : "Sleep timer"
        }
        aria-label="Sleep timer"
        aria-haspopup="menu"
        aria-expanded={anchor !== null}
      >
        <BiMoon />
        {label && <span className="sleep-timer-label">{label}</span>}
      </button>
      {anchor && (
        <ContextMenu anchor={anchor} onClose={() => setAnchor(null)}>
          {SLEEP_CHOICES.map(({ label: choiceLabel, request }) => (
            <button
              key={choiceLabel}
              type="button"
              role="menuitem"
              onClick={() => choose(request)}
            >
              {isCurrent(status, request) ? <BiCheck /> : <BiMoon />}
              {choiceLabel}
            </button>
          ))}
          {active && (
            <button
              type="button"
              role="menuitem"
              onClick={() => choose({ mode: "off" })}
            >
              <BiX /> Turn off
            </button>
          )}
        </ContextMenu>
      )}
    </>
  );
}
