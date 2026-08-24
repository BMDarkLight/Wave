import { useRef, useState } from "react";
import {
  EQ_PRESETS,
  getAutoLyricsDownload,
  getCrossfadeDuration,
  getEqSettings,
  getGaplessEnabled,
  setAutoLyricsDownload,
  setCrossfadeDuration,
  setEqBands,
  setEqEnabled,
  setGaplessEnabled,
  type EqSettings,
} from "../utils/player";
import { formatInvokeError } from "../utils/errors";

export function useEqualizerSettings(setError: (message: string | null) => void) {
  const [showEqPanel, setShowEqPanel] = useState(false);
  const [eqSettings, setEqSettings] = useState<EqSettings>({
    bands: Array(10).fill(0),
    enabled: false,
  });
  const [crossfadeDuration, setCrossfadeDurationState] = useState(0.0);
  const crossfadeSaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [gaplessEnabled, setGaplessEnabledState] = useState(true);
  const [autoLyricsDownload, setAutoLyricsDownloadState] = useState(true);
  const [eqAnchor, setEqAnchor] = useState<{
    bottom: number;
    right: number;
  } | null>(null);
  const volumeIconRef = useRef<HTMLButtonElement>(null);

  const loadEqSettings = async () => {
    try {
      const settings = await getEqSettings();
      const bands = Array.from({ length: 10 }, (_, i) => settings.bands[i] ?? 0);
      setEqSettings({ bands, enabled: settings.enabled });
      const crossfade = await getCrossfadeDuration();
      setCrossfadeDurationState(crossfade);
      const gapless = await getGaplessEnabled();
      setGaplessEnabledState(gapless);
      const autoLyrics = await getAutoLyricsDownload();
      setAutoLyricsDownloadState(autoLyrics);
    } catch (err) {
      console.error("Failed to load EQ settings", err);
    }
  };

  const handleToggleEqPanel = async () => {
    if (showEqPanel) {
      setShowEqPanel(false);
      setEqAnchor(null);
      return;
    }
    await loadEqSettings();
    if (volumeIconRef.current) {
      const rect = volumeIconRef.current.getBoundingClientRect();
      setEqAnchor({
        bottom: window.innerHeight - rect.top + 8,
        right: Math.max(12, window.innerWidth - rect.right),
      });
    }
    setShowEqPanel(true);
  };

  const handleEqEnabled = async (enabled: boolean) => {
    const previous = eqSettings;
    setEqSettings((s) => ({ ...s, enabled }));
    try {
      await setEqEnabled(enabled);
    } catch (err) {
      setEqSettings(previous);
      setError(formatInvokeError(err, "Failed to toggle equalizer"));
    }
  };

  const handleEqBandChange = async (index: number, gain: number) => {
    const bands = eqSettings.bands.map((value, i) => (i === index ? gain : value));
    setEqSettings((s) => ({ ...s, bands, enabled: true }));
    try {
      await setEqBands(bands);
      if (!eqSettings.enabled) await setEqEnabled(true);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to update EQ band"));
      await loadEqSettings();
    }
  };

  const handleEqBandsChange = async (bands: number[]) => {
    setEqSettings((s) => ({ ...s, bands, enabled: true }));
    try {
      await setEqBands(bands);
      if (!eqSettings.enabled) await setEqEnabled(true);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to update equalizer"));
      await loadEqSettings();
    }
  };

  const handleEqPreset = async (presetId: string) => {
    const preset = EQ_PRESETS.find((p) => p.id === presetId);
    if (!preset) return;
    const bands = [...preset.bands];
    setEqSettings({ bands, enabled: true });
    try {
      await setEqBands(bands);
      await setEqEnabled(true);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to apply EQ preset"));
      await loadEqSettings();
    }
  };

  const handleEqReset = async () => {
    await handleEqPreset("flat");
  };

  const handleCrossfadeChange = (duration: number) => {
    const clamped = Math.max(0, Math.min(8, duration));
    setCrossfadeDurationState(clamped);
    if (crossfadeSaveTimer.current) {
      clearTimeout(crossfadeSaveTimer.current);
    }
    crossfadeSaveTimer.current = setTimeout(() => {
      setCrossfadeDuration(clamped).catch(async (err) => {
        setError(formatInvokeError(err, "Failed to set crossfade duration"));
        await loadEqSettings();
      });
    }, 120);
  };

  const handleGaplessChange = async (enabled: boolean) => {
    setGaplessEnabledState(enabled);
    try {
      await setGaplessEnabled(enabled);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to set gapless playback"));
      const gapless = await getGaplessEnabled();
      setGaplessEnabledState(gapless);
    }
  };

  const handleAutoLyricsDownloadChange = async (enabled: boolean) => {
    setAutoLyricsDownloadState(enabled);
    try {
      await setAutoLyricsDownload(enabled);
    } catch (err) {
      setError(formatInvokeError(err, "Failed to update auto lyric download"));
      const autoLyrics = await getAutoLyricsDownload();
      setAutoLyricsDownloadState(autoLyrics);
    }
  };

  return {
    showEqPanel,
    setShowEqPanel,
    eqSettings,
    crossfadeDuration,
    gaplessEnabled,
    autoLyricsDownload,
    eqAnchor,
    setEqAnchor,
    volumeIconRef,
    loadEqSettings,
    handleToggleEqPanel,
    handleEqEnabled,
    handleEqBandChange,
    handleEqBandsChange,
    handleEqPreset,
    handleEqReset,
    handleCrossfadeChange,
    handleGaplessChange,
    handleAutoLyricsDownloadChange,
  };
}
