/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { createPortal } from "react-dom";
import { BiX } from "react-icons/bi";
import { formatTime } from "../utils/format";
import SpeedRow from "./SpeedRow";
import {
  EQ_BAND_LABELS,
  EQ_PRESETS,
  SLEEP_CHOICES,
  type EqSettings,
  type SleepRequest,
  type SleepTimerStatus,
} from "../utils/player";

/** The select's value for the timer as it stands. */
function sleepSelectValue(status: SleepTimerStatus): string {
  if (status.mode === "off") return "off";
  if (status.mode === "countdown") return "countdown";
  return String(
    SLEEP_CHOICES.findIndex(({ request }) => request.mode === "end_of_track"),
  );
}

function sleepRequestFor(value: string): SleepRequest {
  if (value === "off") return { mode: "off" };
  return SLEEP_CHOICES[Number(value)].request;
}

export default function EqPanel({
  anchor,
  onClose,
  settings,
  onEnabledChange,
  onPresetSelect,
  onReset,
  onBandChange,
  crossfadeDuration,
  onCrossfadeChange,
  gaplessEnabled,
  onGaplessChange,
  sleepTimer,
  onSleepTimerChange,
  speed,
  onSpeedChange,
}: {
  anchor: { bottom: number; right: number };
  onClose: () => void;
  settings: EqSettings;
  onEnabledChange: (enabled: boolean) => void;
  onPresetSelect: (presetId: string) => void;
  onReset: () => void;
  onBandChange: (index: number, gain: number) => void;
  crossfadeDuration: number;
  onCrossfadeChange: (duration: number) => void;
  gaplessEnabled: boolean;
  onGaplessChange: (enabled: boolean) => void;
  sleepTimer: SleepTimerStatus;
  onSleepTimerChange: (request: SleepRequest) => void;
  speed: number;
  onSpeedChange: (speed: number) => void;
}) {
  return createPortal(
    <>
      <div className="context-menu-backdrop" onClick={onClose} />
      <div
        className="eq-panel"
        style={{
          position: "fixed",
          bottom: `${anchor.bottom}px`,
          right: `${anchor.right}px`,
        }}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="Equalizer"
      >
        <div className="eq-panel-header">
          <h3>Equalizer</h3>
          <label className="eq-enable">
            <input
              type="checkbox"
              checked={settings.enabled}
              onChange={(event) => onEnabledChange(event.target.checked)}
            />
            On
          </label>
          <button
            className="eq-close"
            onClick={onClose}
            type="button"
            title="Close"
            aria-label="Close equalizer"
          >
            <BiX />
          </button>
        </div>
        <div className="eq-panel-toolbar">
          <select
            className="eq-preset-select"
            value=""
            onChange={(event) => {
              if (event.target.value) onPresetSelect(event.target.value);
            }}
            aria-label="EQ preset"
          >
            <option value="" disabled>
              Presets
            </option>
            {EQ_PRESETS.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.label}
              </option>
            ))}
          </select>
          <button className="btn-ghost btn-sm" onClick={onReset} type="button">
            Reset
          </button>
        </div>
        <div className={`eq-bands ${settings.enabled ? "" : "disabled"}`}>
          {EQ_BAND_LABELS.map((label, index) => (
            <div className="eq-band" key={label}>
              <span className="eq-band-gain">
                {(settings.bands[index] ?? 0) > 0 ? "+" : ""}
                {(settings.bands[index] ?? 0).toFixed(0)}
              </span>
              <input
                type="range"
                min={-12}
                max={12}
                step={0.5}
                value={settings.bands[index] ?? 0}
                onChange={(event) =>
                  onBandChange(index, Number(event.target.value))
                }
                aria-label={`${label} Hz`}
                title={`${label} Hz`}
              />
              <span className="eq-band-label">{label}</span>
            </div>
          ))}
        </div>
        <div
          className="eq-crossfade"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => event.stopPropagation()}
        >
          <label className="eq-crossfade-label">Crossfade</label>
          <input
            type="range"
            min={0}
            max={8}
            step={0.5}
            value={crossfadeDuration}
            onChange={(event) => onCrossfadeChange(Number(event.target.value))}
            aria-label="Crossfade duration in seconds"
          />
          <span className="eq-crossfade-value">
            {crossfadeDuration === 0
              ? "Off"
              : `${crossfadeDuration.toFixed(1)}s`}
          </span>
        </div>
        <div
          className="eq-crossfade eq-subrow"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => event.stopPropagation()}
        >
          <label className="eq-crossfade-label" htmlFor="eq-sleep-select">
            Sleep
          </label>
          <select
            id="eq-sleep-select"
            className="eq-preset-select"
            value={sleepSelectValue(sleepTimer)}
            onChange={(event) =>
              onSleepTimerChange(sleepRequestFor(event.target.value))
            }
          >
            <option value="off">Off</option>
            {sleepTimer.mode === "countdown" && (
              <option value="countdown" disabled hidden>
                Counting down
              </option>
            )}
            {SLEEP_CHOICES.map(({ label }, index) => (
              <option key={label} value={index}>
                {label}
              </option>
            ))}
          </select>
          <span className="eq-crossfade-value">
            {sleepTimer.mode === "countdown"
              ? formatTime(Math.ceil(sleepTimer.remaining_seconds ?? 0))
              : sleepTimer.mode === "end_of_track"
                ? "End"
                : "Off"}
          </span>
        </div>
        <SpeedRow id="eq-speed-range" speed={speed} onChange={onSpeedChange} />
        <label
          className="eq-gapless"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={(event) => event.stopPropagation()}
        >
          <input
            type="checkbox"
            checked={gaplessEnabled}
            onChange={(event) => onGaplessChange(event.target.checked)}
          />
          <span className="eq-gapless-copy">
            <span className="eq-gapless-label">Gapless playback</span>
            <span className="eq-gapless-hint">
              Play consecutive tracks without silence between them.
            </span>
          </span>
        </label>
      </div>
    </>,
    document.body,
  );
}
