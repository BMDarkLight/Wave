/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  checkForUpdate,
  downloadUpdate,
  getAppVersion,
  getAutoUpdateEnabled,
  installUpdate,
  onUpdateProgress,
  openReleasePage,
  setAutoUpdateEnabled,
  type UpdateCheck,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "downloading"
  | "ready"
  | "error";

// Leave the first seconds after launch to the library and playback.
const LAUNCH_CHECK_DELAY_MS = 4000;

/**
 * Looks for a newer Wave release. With auto update on, that happens at launch,
 * and on Android the APK is downloaded too, ready to install. With it off, the
 * Settings page offers a manual check.
 */
export function useAppUpdate(setError: (message: string | null) => void) {
  const [autoUpdate, setAutoUpdate] = useState(true);
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [check, setCheck] = useState<UpdateCheck | null>(null);
  const [currentVersion, setCurrentVersion] = useState<string | null>(null);
  /** 0 to 1 while downloading. */
  const [progress, setProgress] = useState<number | null>(null);
  /** Why the last check or download failed, shown in Settings. */
  const [problem, setProblem] = useState<string | null>(null);
  const [toastDismissed, setToastDismissed] = useState(false);
  const busy = useRef(false);

  useEffect(() => {
    getAppVersion()
      .then(setCurrentVersion)
      .catch(() => {});
  }, []);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    onUpdateProgress(({ received, total }) => {
      setProgress(total > 0 ? Math.min(received / total, 1) : null);
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const download = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setStatus("downloading");
    setProgress(0);
    setProblem(null);
    try {
      await downloadUpdate();
      setStatus("ready");
      setToastDismissed(false);
    } catch (err) {
      setStatus("available");
      setProblem(formatInvokeError(err, "Could not download the update"));
    } finally {
      setProgress(null);
      busy.current = false;
    }
  }, []);

  /** With `fetch`, also download the APK when this device has one. */
  const runCheck = useCallback(
    async (fetch: boolean) => {
      if (busy.current) return;
      busy.current = true;
      setStatus("checking");
      setProblem(null);
      let result: UpdateCheck;
      try {
        result = await checkForUpdate();
      } catch (err) {
        setStatus("error");
        setProblem(formatInvokeError(err, "Could not check for updates"));
        return;
      } finally {
        busy.current = false;
      }
      setCheck(result);
      setCurrentVersion(result.current_version);
      setToastDismissed(false);
      if (!result.latest_version) setStatus("current");
      else if (result.downloaded) setStatus("ready");
      else if (fetch && result.download_size !== null) await download();
      else setStatus("available");
    },
    [download],
  );

  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;
    getAutoUpdateEnabled()
      .catch(() => true)
      .then((enabled) => {
        if (cancelled) return;
        setAutoUpdate(enabled);
        if (enabled) {
          timer = window.setTimeout(
            () => void runCheck(true),
            LAUNCH_CHECK_DELAY_MS,
          );
        }
      });
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [runCheck]);

  const handleAutoUpdateChange = async (enabled: boolean) => {
    setAutoUpdate(enabled);
    try {
      await setAutoUpdateEnabled(enabled);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to update the setting"));
      setAutoUpdate(!enabled);
      return;
    }
    if (!enabled) return;
    if (status === "idle" || status === "error" || status === "current") {
      void runCheck(true);
    } else if (status === "available" && check?.download_size != null) {
      void download();
    }
  };

  const install = async () => {
    try {
      await installUpdate();
    } catch (err) {
      setError(formatInvokeError(err, "Could not open the installer"));
    }
  };

  const openPage = async () => {
    try {
      await openReleasePage();
    } catch (err) {
      setError(formatInvokeError(err, "Could not open the release page"));
    }
  };

  // Only what the launch check finds is announced outside Settings: a manual
  // check is made from Settings, which already shows the answer.
  const showToast =
    autoUpdate &&
    !toastDismissed &&
    (status === "ready" ||
      (status === "available" && check?.download_size == null));

  return {
    autoUpdate,
    handleAutoUpdateChange,
    status,
    currentVersion,
    latestVersion: check?.latest_version ?? null,
    canDownload: check?.download_size != null,
    downloadSize: check?.download_size ?? null,
    progress,
    problem,
    checkNow: () => void runCheck(false),
    download: () => void download(),
    install: () => void install(),
    openPage: () => void openPage(),
    showToast,
    dismissToast: () => setToastDismissed(true),
  };
}

export type AppUpdate = ReturnType<typeof useAppUpdate>;
