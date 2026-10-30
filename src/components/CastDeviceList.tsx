/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import {
  BiChevronRight,
  BiDesktop,
  BiDevices,
  BiHeadphone,
  BiMobile,
} from "react-icons/bi";
import type { WavePeer } from "../utils/peers";

/**
 * Where music plays: this device and its outputs, and the paired Waves on
 * the network. Picking a Wave plays there; this device stays the remote.
 */
export default function CastDeviceList({
  onPhone,
  outputs,
  currentOutput,
  castDevice,
  peers,
  sourcingOn,
  busy,
  onPickOutput,
  onPickPeer,
  onPair,
  onManage,
}: {
  onPhone: boolean;
  /** This device's own audio outputs; empty where the OS picks one. */
  outputs: string[];
  currentOutput: string;
  castDevice: { id: string; name: string } | null;
  peers: WavePeer[];
  /** Outside Sourcing, without which no other Wave is looked for. */
  sourcingOn: boolean;
  /** A device being switched to. */
  busy: string | null;
  /** Play here, on `name`, or on the current output when null. */
  onPickOutput: (name: string | null) => void;
  onPickPeer: (peer: WavePeer) => void;
  onPair: (peer: WavePeer) => void;
  /** Open the Devices page. */
  onManage: () => void;
}) {
  const here = onPhone ? "This phone" : "This computer";
  const HereIcon = onPhone ? BiMobile : BiDesktop;
  const others = outputs.filter(
    (name) => castDevice !== null || name !== currentOutput,
  );
  const waves = [...peers].sort(
    (a, b) =>
      Number(b.paired) - Number(a.paired) ||
      Number(b.online) - Number(a.online) ||
      a.name.localeCompare(b.name),
  );

  return (
    <div className="cast-devices">
      <div className="cast-current">
        {castDevice ? (
          <BiDevices className="cast-current-icon" aria-hidden />
        ) : currentOutput && !onPhone ? (
          <BiHeadphone className="cast-current-icon" aria-hidden />
        ) : (
          <HereIcon className="cast-current-icon" aria-hidden />
        )}
        <span className="cast-text">
          <span className="cast-current-name">
            {castDevice ? castDevice.name : currentOutput || here}
          </span>
          <span className="cast-hint">
            {castDevice
              ? "Wave device"
              : currentOutput && !onPhone
                ? here
                : "Playing here"}
          </span>
        </span>
      </div>

      {castDevice && (
        <button
          className="cast-row"
          type="button"
          disabled={busy !== null}
          onClick={() => onPickOutput(null)}
        >
          <HereIcon className="cast-row-icon" aria-hidden />
          <span className="cast-text">
            <span className="cast-name">{here}</span>
            <span className="cast-hint">
              {busy === "here" ? "Switching…" : "Play here again"}
            </span>
          </span>
        </button>
      )}
      {others.map((name) => (
        <button
          key={name}
          className="cast-row"
          type="button"
          disabled={busy !== null}
          onClick={() => onPickOutput(name)}
        >
          <BiHeadphone className="cast-row-icon" aria-hidden />
          <span className="cast-text">
            <span className="cast-name">{name}</span>
            {busy === name && <span className="cast-hint">Switching…</span>}
          </span>
        </button>
      ))}

      <p className="cast-section">Wave devices</p>
      {!sourcingOn ? (
        <p className="cast-empty">
          Turn on Outside Sourcing in Settings to play on your other Waves.
        </p>
      ) : waves.length === 0 ? (
        <p className="cast-empty">
          Other Waves on this network show up here when Outside Sourcing is on
          for them too.
        </p>
      ) : (
        waves.map((peer) => {
          const current = castDevice?.id === peer.id;
          if (!peer.paired) {
            return (
              <div key={peer.id} className="cast-row cast-row--static">
                <BiDevices className="cast-row-icon" aria-hidden />
                <span className="cast-text">
                  <span className="cast-name">{peer.name}</span>
                  <span className="cast-hint">
                    {peer.asked
                      ? "Waiting for it to answer…"
                      : peer.asking
                        ? "Wants to share libraries"
                        : "Pair to play there"}
                  </span>
                </span>
                {!peer.asked && !peer.asking && (
                  <button
                    className="btn-ghost btn-sm"
                    type="button"
                    onClick={() => onPair(peer)}
                  >
                    Pair
                  </button>
                )}
              </div>
            );
          }
          return (
            <button
              key={peer.id}
              className={`cast-row${current ? " active" : ""}`}
              type="button"
              disabled={!peer.online || current || busy !== null}
              onClick={() => onPickPeer(peer)}
            >
              <BiDevices className="cast-row-icon" aria-hidden />
              <span className="cast-text">
                <span className="cast-name">
                  <span
                    className={`wave-device-dot${peer.online ? " online" : ""}`}
                    aria-hidden
                  />
                  {peer.name}
                </span>
                <span className="cast-hint">
                  {busy === peer.id
                    ? "Connecting…"
                    : current
                      ? "Playing there"
                      : peer.online
                        ? "Available"
                        : "Offline"}
                </span>
              </span>
            </button>
          );
        })
      )}
      {sourcingOn && (
        <button className="cast-manage" type="button" onClick={onManage}>
          Pair and manage devices
          <BiChevronRight aria-hidden />
        </button>
      )}
    </div>
  );
}
