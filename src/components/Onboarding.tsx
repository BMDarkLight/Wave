/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import { BiFolderOpen } from "react-icons/bi";
import SourcingChoice from "./SourcingChoice";
import type { ImportProgress } from "../hooks/useMediaImport";

const count = new Intl.NumberFormat();

/** The wave in Wave's logo. */
const WAVE_PATH =
  "M410.15,330.58l-4.16-12.39c-9.18-27.22-18.64-46.46-28.43-57.73-9.68-11.28-22.44-15.76-38.16-13.39-11.71,1.76-22.69,6.92-32.89,15.45-10.19,8.59-20.98,20.11-32.31,34.56-12.27,15.13-25.05,27.17-38.34,36.23-13.35,9.01-28.5,14.79-45.45,17.34-32.94,4.96-58.93-1.74-78.04-20.02-19.1-18.29-35.11-46.88-48.13-85.77l-10.35-30.59,65.5-9.86,4.16,12.39c9.18,27.22,18.21,46.52,27.09,57.93,8.88,11.4,19.66,16.18,32.39,14.26,11.5-1.73,21.67-6.82,30.61-15.33,8.94-8.51,18.76-19.56,29.46-33.14,11.97-15.31,25.15-27.63,39.39-36.93,14.34-9.32,30.34-15.29,48.04-17.95,36.41-5.48,64.77.58,85.03,18.2,20.26,17.62,36.65,45.12,49.17,82.55l11.66,34.22-66.25,9.97Z";

/** How long the logo plays before the first question comes in. */
const INTRO_MS = 1900;
/** How long the screen takes to fade out over the app. */
const LEAVE_MS = 450;

type Step = "welcome" | "music" | "importing" | "sourcing";

function WaveMark({ className }: { className: string }) {
  return (
    <svg className={className} viewBox="50 180 430 176" aria-hidden>
      <path d={WAVE_PATH} />
    </svg>
  );
}

/**
 * The first run: Wave's logo draws itself, then a few questions, then the
 * app. Also shown again after a reset.
 */
export default function Onboarding({
  animate,
  askForMusic,
  onPickMusic,
  progress,
  error,
  onLeaveImport,
  onFinish,
  onDone,
}: {
  /** False with Reduce motion or animations off: no logo, no fades. */
  animate: boolean;
  /** The library is empty, so offer to add a folder. */
  askForMusic: boolean;
  /** Opens the folder picker and imports what was picked. */
  onPickMusic: () => void;
  /** The import the picked folder started, while there is one to show. */
  progress: ImportProgress | null;
  /** Why the picked folder didn't import, shown on the music step. */
  error: string | null;
  /** Stop showing the import; it carries on in the background. */
  onLeaveImport: () => void;
  /** Saves the answers, as soon as the last one is in. */
  onFinish: (outsideSourcing: boolean) => void;
  /** The screen is gone and the app shows. */
  onDone: () => void;
}) {
  const [intro, setIntro] = useState(animate);
  const [leaving, setLeaving] = useState(false);
  const [step, setStep] = useState<Step>("welcome");
  const [outside, setOutside] = useState(false);
  const steps: Step[] = askForMusic
    ? ["welcome", "music", "sourcing"]
    : ["welcome", "sourcing"];
  // The import belongs to the music step as far as the dots go.
  const at = steps.indexOf(step === "importing" ? "music" : step);

  // A folder was picked: follow its import. A cancelled picker leaves the
  // question where it was.
  useEffect(() => {
    if (step === "music" && progress) setStep("importing");
  }, [step, progress]);

  useEffect(() => {
    if (!intro) return;
    const timer = window.setTimeout(() => setIntro(false), INTRO_MS);
    return () => window.clearTimeout(timer);
  }, [intro]);

  const next = () => setStep(steps[Math.min(at + 1, steps.length - 1)]);

  const leaveImport = () => {
    onLeaveImport();
    setStep("sourcing");
  };

  const finish = () => {
    onFinish(outside);
    if (!animate) {
      onDone();
      return;
    }
    setLeaving(true);
    window.setTimeout(onDone, LEAVE_MS);
  };

  return (
    <div
      className={`oobe${intro ? " oobe--intro" : " oobe--steps"}${leaving ? " oobe--leaving" : ""}${animate ? "" : " oobe--still"}`}
      role="dialog"
      aria-modal="true"
      aria-label="Welcome to Wave"
    >
      {animate && (
        <div className="oobe-intro" aria-hidden>
          <WaveMark className="oobe-intro-mark" />
        </div>
      )}

      <div className="oobe-card">
        <WaveMark className="oobe-card-mark" />
        <div key={step} className="oobe-step">
          {step === "welcome" && (
            <>
              <h1>Welcome to Wave</h1>
              <p>
                A music player for the songs you own. It plays your files where
                they are, learns what you like, and keeps everything on your
                device.
              </p>
              <div className="oobe-actions">
                <button className="btn-primary" type="button" onClick={next}>
                  Get started
                </button>
              </div>
            </>
          )}
          {step === "music" && (
            <>
              <h1>Add your music</h1>
              <p>
                Pick a folder with your music. Wave reads the files in place and
                copies nothing. You can add more folders later.
              </p>
              <div className="oobe-actions">
                <button className="btn-ghost" type="button" onClick={next}>
                  Skip for now
                </button>
                <button
                  className="btn-primary"
                  type="button"
                  onClick={onPickMusic}
                >
                  <BiFolderOpen aria-hidden /> Choose a folder
                </button>
              </div>
              {error && (
                <p className="oobe-error" role="alert">
                  {error}
                </p>
              )}
            </>
          )}
          {step === "importing" && (
            <ImportStep
              progress={progress}
              onBackground={leaveImport}
              onContinue={leaveImport}
            />
          )}
          {step === "sourcing" && (
            <>
              <h1>Beyond this device</h1>
              <SourcingChoice enabled={outside} onChange={setOutside} />
              <div className="oobe-actions">
                <button className="btn-primary" type="button" onClick={finish}>
                  Start listening
                </button>
              </div>
            </>
          )}
        </div>
        <div className="oobe-dots" aria-hidden>
          {steps.map((s, i) => (
            <span key={s} className={i <= at ? "on" : ""} />
          ))}
        </div>
      </div>
    </div>
  );
}

/** The picked folder coming in: found, added one by one, then done. */
function ImportStep({
  progress,
  onBackground,
  onContinue,
}: {
  progress: ImportProgress | null;
  onBackground: () => void;
  onContinue: () => void;
}) {
  const phase = progress?.phase ?? "finding";
  const total = progress?.total ?? 0;
  const processed = progress?.processed ?? 0;
  const added = progress?.added ?? 0;
  const fraction = total > 0 ? Math.min(processed / total, 1) : 0;
  const done = phase === "done";

  return (
    <>
      <h1>
        {done
          ? "Your Library is ready"
          : phase === "finding"
            ? "Finding your music"
            : "Adding your songs"}
      </h1>
      <p aria-live="polite">
        {done
          ? added === 1
            ? "1 song added."
            : added > 0
              ? `${count.format(added)} songs added.`
              : "Everything in that folder is already in your Library."
          : phase === "finding" || total === 0
            ? "Looking through the folder you picked…"
            : `${count.format(processed)} of ${count.format(total)}`}
      </p>
      {!done && (
        <div
          className={`import-progress-bar oobe-progress${phase === "finding" || total === 0 ? " indeterminate" : ""}`}
          role="progressbar"
          aria-label="Importing your music"
          aria-valuemin={0}
          aria-valuemax={total || undefined}
          aria-valuenow={total ? processed : undefined}
        >
          <span style={{ width: `${fraction * 100}%` }} />
        </div>
      )}
      <div className="oobe-actions">
        {done ? (
          <button className="btn-primary" type="button" onClick={onContinue}>
            Continue
          </button>
        ) : (
          <button
            className="oobe-background"
            type="button"
            onClick={onBackground}
          >
            Keep going in the background
          </button>
        )}
      </div>
    </>
  );
}
