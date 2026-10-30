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
import {
  BiChevronLeft,
  BiDevices,
  BiPlayCircle,
  BiRefresh,
  BiWifi,
} from "react-icons/bi";
import type { SourceSettings } from "../utils/player";
import {
  answerWaveRequest,
  connectWavePeer,
  forgetWavePeer,
  setWaveShare,
  type PeerEvent,
  type WavePeer,
} from "../utils/peers";
import { formatInvokeError } from "../utils/errors";
import LibrarySharingRow from "./LibrarySharingRow";

function shareLine(peer: WavePeer): string {
  switch (peer.share) {
    case "everything":
      return "Sees your whole library";
    case "songs":
      return `Sees ${peer.share_count} ${peer.share_count === 1 ? "song" : "songs"} of yours`;
    default:
      return "Sees none of your songs";
  }
}

/**
 * Everything about other Waves in one place: finding them, pairing, what
 * each may see, playing on them, and how this one shows up to them.
 */
export default function WaveDevicesPage({
  peers,
  searching,
  onSearch,
  onChanged,
  onChoose,
  setError,
  sourceSettings,
  onSourceSettingsChange,
  castDeviceId,
  onPlayThere,
  onOpenPlaylist,
  onBack,
}: {
  peers: WavePeer[];
  searching: boolean;
  onSearch: () => void;
  onChanged: () => void;
  /** Open the song picker to choose what `peer` sees; `answering` while
   *  its request still waits. */
  onChoose: (peer: PeerEvent, answering: boolean) => void;
  setError: (message: string | null) => void;
  sourceSettings: SourceSettings;
  onSourceSettingsChange: (next: SourceSettings) => void;
  /** The paired Wave music plays on, if any. */
  castDeviceId: string | null;
  onPlayThere: (peer: WavePeer) => void;
  onOpenPlaylist: (id: string) => void;
  /** Back to the Library tab. Only shown on the phone layout. */
  onBack: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const on = sourceSettings.remote_sources_enabled;

  const run = async (
    id: string,
    work: () => Promise<void>,
    failure: string,
  ) => {
    setBusy(id);
    try {
      await work();
    } catch (err) {
      setError(formatInvokeError(err, failure));
    } finally {
      setBusy(null);
      onChanged();
    }
  };

  const byName = (a: WavePeer, b: WavePeer) => a.name.localeCompare(b.name);
  const asking = peers.filter((p) => !p.paired && p.asking).sort(byName);
  const paired = peers
    .filter((p) => p.paired)
    .sort((a, b) => Number(b.online) - Number(a.online) || byName(a, b));
  const nearby = peers.filter((p) => !p.paired && !p.asking).sort(byName);

  return (
    <main className="main-content playlist-page wave-devices-page">
      <div className="hero-copy">
        <div className="hero-top">
          <button
            className="mlib-icon-btn playlist-back-btn"
            onClick={onBack}
            type="button"
            title="Back"
            aria-label="Back to library"
          >
            <BiChevronLeft />
          </button>
          <h1>Devices</h1>
          {on && (
            <div className="hero-actions">
              <button
                className={`btn-secondary wdev-search${searching ? " searching" : ""}`}
                onClick={onSearch}
                disabled={searching}
                type="button"
                title={searching ? "Searching…" : "Search again"}
                aria-label={
                  searching ? "Searching for other Waves" : "Search again"
                }
              >
                <BiRefresh aria-hidden />
              </button>
            </div>
          )}
        </div>
        <p>Share music and play on your other Waves on this network.</p>
      </div>

      <section className="wdev-body">
        {!on ? (
          <div className="wdev-card wdev-off">
            <BiDevices className="wdev-off-icon" aria-hidden />
            <h2>Devices are off</h2>
            <p>
              Turn on Outside Sourcing to find your other Waves on this network
              and let them find this one. Wave connects to nothing while it is
              off.
            </p>
            <button
              className="btn-primary"
              type="button"
              onClick={() =>
                onSourceSettingsChange({
                  ...sourceSettings,
                  remote_sources_enabled: true,
                })
              }
            >
              Turn on Outside Sourcing
            </button>
          </div>
        ) : (
          <>
            {asking.length > 0 && (
              <>
                <h2 className="wdev-section">Wants to share</h2>
                {asking.map((peer) => (
                  <div key={peer.id} className="wdev-card wdev-asking">
                    <DeviceHead peer={peer} hint="Asks to share libraries" />
                    <div className="wdev-actions">
                      <button
                        className="btn-primary btn-sm"
                        type="button"
                        disabled={busy === peer.id}
                        onClick={() =>
                          void run(
                            peer.id,
                            () =>
                              answerWaveRequest(peer.id, true, {
                                kind: "everything",
                              }),
                            `Couldn't answer ${peer.name}`,
                          )
                        }
                      >
                        Share everything
                      </button>
                      <button
                        className="btn-ghost btn-sm"
                        type="button"
                        disabled={busy === peer.id}
                        onClick={() => onChoose(peer, true)}
                      >
                        Choose…
                      </button>
                      <button
                        className="btn-ghost btn-sm"
                        type="button"
                        disabled={busy === peer.id}
                        onClick={() =>
                          void run(
                            peer.id,
                            () =>
                              answerWaveRequest(peer.id, false, {
                                kind: "nothing",
                              }),
                            `Couldn't answer ${peer.name}`,
                          )
                        }
                      >
                        Not now
                      </button>
                    </div>
                  </div>
                ))}
              </>
            )}

            {paired.length > 0 && (
              <>
                <h2 className="wdev-section">Paired</h2>
                {paired.map((peer) => {
                  const here = castDeviceId === peer.id;
                  return (
                    <div key={peer.id} className="wdev-card">
                      <DeviceHead
                        peer={peer}
                        hint={`${peer.online ? "Available" : "Offline"} · ${shareLine(peer)}`}
                        live={
                          here
                            ? "Playing there"
                            : peer.playing_from_me
                              ? "Playing from you"
                              : null
                        }
                      />
                      <div className="wdev-actions">
                        {peer.online && (
                          <button
                            className="btn-primary btn-sm wdev-play"
                            type="button"
                            disabled={here || busy !== null}
                            onClick={() => onPlayThere(peer)}
                          >
                            <BiPlayCircle aria-hidden />
                            {here ? "Playing there" : "Play there"}
                          </button>
                        )}
                        {peer.playlist_id && (
                          <button
                            className="btn-ghost btn-sm"
                            type="button"
                            onClick={() =>
                              onOpenPlaylist(peer.playlist_id as string)
                            }
                          >
                            Its songs
                          </button>
                        )}
                        <button
                          className="btn-ghost btn-sm"
                          type="button"
                          disabled={busy === peer.id}
                          onClick={() => onChoose(peer, false)}
                        >
                          Choose what it sees
                        </button>
                        {peer.share !== "everything" && (
                          <button
                            className="btn-ghost btn-sm"
                            type="button"
                            disabled={busy === peer.id}
                            onClick={() =>
                              void run(
                                peer.id,
                                () =>
                                  setWaveShare(peer.id, { kind: "everything" }),
                                `Couldn't change what ${peer.name} sees`,
                              )
                            }
                          >
                            Share all
                          </button>
                        )}
                        <button
                          className="btn-ghost btn-sm wave-device-forget"
                          type="button"
                          disabled={busy === peer.id}
                          onClick={() =>
                            void run(
                              peer.id,
                              () => forgetWavePeer(peer.id),
                              `Couldn't forget ${peer.name}`,
                            )
                          }
                        >
                          Forget
                        </button>
                      </div>
                    </div>
                  );
                })}
              </>
            )}

            <h2 className="wdev-section">On this network</h2>
            {nearby.map((peer) => (
              <div key={peer.id} className="wdev-card">
                <DeviceHead
                  peer={peer}
                  hint={
                    peer.asked ? "Waiting for it to answer…" : "Not paired yet"
                  }
                />
                {!peer.asked && (
                  <div className="wdev-actions">
                    <button
                      className="btn-primary btn-sm"
                      type="button"
                      disabled={busy === peer.id}
                      onClick={() =>
                        void run(
                          peer.id,
                          () => connectWavePeer(peer.id),
                          `Couldn't reach ${peer.name}`,
                        )
                      }
                    >
                      Share libraries
                    </button>
                  </div>
                )}
              </div>
            ))}
            {searching ? (
              <div className="wdev-searching" role="status">
                <span className="import-spinner" aria-hidden />
                Looking for other Waves…
              </div>
            ) : (
              nearby.length === 0 && (
                <div className="wdev-card wdev-tips">
                  <BiWifi className="wdev-tips-icon" aria-hidden />
                  <div>
                    <p className="wdev-tips-title">
                      {paired.length > 0
                        ? "No other Waves nearby"
                        : "No other Waves found yet"}
                    </p>
                    <ul>
                      <li>
                        Open Wave on the other device and turn on Outside
                        Sourcing there too.
                      </li>
                      <li>Keep both on the same Wi-Fi or hotspot.</li>
                      <li>
                        A VPN or a guest network can keep devices from seeing
                        each other.
                      </li>
                    </ul>
                  </div>
                </div>
              )
            )}

            <h2 className="wdev-section">This device</h2>
            <div className="wdev-card wdev-self">
              <LibrarySharingRow
                sourceSettings={sourceSettings}
                onSourceSettingsChange={onSourceSettingsChange}
                idPrefix="wdev-"
              />
            </div>
          </>
        )}
      </section>
    </main>
  );
}

function DeviceHead({
  peer,
  hint,
  live,
}: {
  peer: WavePeer;
  hint: string;
  live?: string | null;
}) {
  return (
    <div className="wdev-head">
      <span className="wdev-icon" aria-hidden>
        <BiDevices />
      </span>
      <span className="wdev-text">
        <span className="wdev-name">
          <span
            className={`wave-device-dot${peer.online ? " online" : ""}`}
            title={peer.online ? "Available" : "Offline"}
            aria-label={peer.online ? "Available" : "Offline"}
          />
          {peer.name}
          {live && <span className="wave-device-live">{live}</span>}
        </span>
        <span className="wdev-hint">{hint}</span>
      </span>
    </div>
  );
}
