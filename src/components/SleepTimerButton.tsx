/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { BiCheck, BiMoon, BiX } from "react-icons/bi";
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

/** The line under the popover's title. */
function sleepSummary(status: SleepTimerStatus): string {
  if (status.mode === "countdown") return `Pauses in ${sleepLabel(status)}`;
  if (status.mode === "end_of_track") return "Pauses when this track ends";
  return "Pause playback after";
}

function isCurrent(status: SleepTimerStatus, request: SleepRequest): boolean {
  // A countdown is always partway through, so only end-of-track can match.
  return request.mode === "end_of_track" && status.mode === "end_of_track";
}

/** Gap between the button and the popover above it. */
const GAP = 10;
/** The popover never comes closer than this to the screen's sides. */
const GUTTER = 16;
/** Matches the close transition in the stylesheet. */
const CLOSE_MS = 240;

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
  // The button's rect while the popover is mounted. Kept through the close
  // so the popover shrinks back into the same spot it grew from.
  const [origin, setOrigin] = useState<DOMRect | null>(null);
  const [open, setOpen] = useState(false);
  const label = sleepLabel(status);
  const active = status.mode !== "off";

  useEffect(() => {
    if (!origin) return;
    if (open) {
      const onKey = (event: KeyboardEvent) => {
        if (event.key === "Escape") setOpen(false);
      };
      window.addEventListener("keydown", onKey);
      return () => window.removeEventListener("keydown", onKey);
    }
    const timer = window.setTimeout(() => setOrigin(null), CLOSE_MS);
    return () => window.clearTimeout(timer);
  }, [origin, open]);

  const show = (rect: DOMRect) => {
    setOrigin(rect);
    // Two frames so the closed state paints before it transitions open.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => setOpen(true));
    });
  };

  const choose = (request: SleepRequest) => {
    setOpen(false);
    onSet(request);
  };

  let popoverStyle: CSSProperties | undefined;
  if (origin) {
    const width = Math.min(300, window.innerWidth - GUTTER * 2);
    const centerX = origin.left + origin.width / 2;
    const left = (window.innerWidth - width) / 2;
    popoverStyle = {
      left,
      width,
      bottom: window.innerHeight - origin.top + GAP,
      // Grow out of the middle of the moon, which sits below the box.
      transformOrigin: `${centerX - left}px calc(100% + ${GAP + origin.height / 2}px)`,
    };
  }

  return (
    <>
      <button
        className={`${className} sleep-timer-btn ${active || open ? "active" : ""}`}
        onClick={(event) => {
          if (open) setOpen(false);
          else show(event.currentTarget.getBoundingClientRect());
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
        aria-haspopup="dialog"
        aria-expanded={open}
      >
        <BiMoon />
        {label && <span className="sleep-timer-label">{label}</span>}
      </button>
      {origin &&
        createPortal(
          <>
            <div
              className="sleep-popover-backdrop"
              onClick={() => setOpen(false)}
            />
            <div
              className={`sleep-popover${open ? " sleep-popover-open" : ""}`}
              style={popoverStyle}
              role="dialog"
              aria-label="Sleep timer"
            >
              <div className="sleep-popover-head">
                <BiMoon />
                <div>
                  <h3>Sleep timer</h3>
                  <p>{sleepSummary(status)}</p>
                </div>
              </div>
              <div className="sleep-popover-grid">
                {SLEEP_CHOICES.map(({ label: choiceLabel, request }) => (
                  <button
                    key={choiceLabel}
                    type="button"
                    className={isCurrent(status, request) ? "current" : ""}
                    onClick={() => choose(request)}
                  >
                    {isCurrent(status, request) && <BiCheck />}
                    {choiceLabel}
                  </button>
                ))}
              </div>
              {active && (
                <button
                  type="button"
                  className="sleep-popover-off"
                  onClick={() => choose({ mode: "off" })}
                >
                  <BiX /> Turn off
                </button>
              )}
            </div>
          </>,
          document.body,
        )}
    </>
  );
}
