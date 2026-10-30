/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

/**
 * The first-run question about Outside Sourcing: keep Wave to this device,
 * or let it reach your servers and your other Waves too.
 */
export default function SourcingChoice({
  enabled,
  onChange,
}: {
  enabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <fieldset className="sourcing-choice">
      <legend>Where should Wave find music?</legend>
      <label className={`sourcing-option${!enabled ? " picked" : ""}`}>
        <input
          type="radio"
          name="sourcing-choice"
          checked={!enabled}
          onChange={() => onChange(false)}
        />
        <span className="sourcing-copy">
          <span className="sourcing-label">Only on this device</span>
          <span className="sourcing-hint">
            Wave plays your files and doesn't connect to anything.
          </span>
        </span>
      </label>
      <label className={`sourcing-option${enabled ? " picked" : ""}`}>
        <input
          type="radio"
          name="sourcing-choice"
          checked={enabled}
          onChange={() => onChange(true)}
        />
        <span className="sourcing-copy">
          <span className="sourcing-label">Also with Outside Sourcing</span>
          <span className="sourcing-hint">
            Play from your own servers (Plex, Jellyfin, Subsonic, WebDAV, DLNA,
            Kodi, Audius) and share music with your other Waves.
          </span>
        </span>
      </label>
      <p className="sourcing-note">
        With Outside Sourcing on, Wave searches your local network for other
        Waves, and they can see this one by its name. Nothing outside your
        network can reach it. You can change this any time in Settings.
      </p>
    </fieldset>
  );
}
