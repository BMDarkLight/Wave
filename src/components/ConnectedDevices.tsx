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
import { BiDevices } from "react-icons/bi";
import {
  answerWaveRequest,
  connectWavePeer,
  forgetWavePeer,
  setWaveShare,
  type PeerEvent,
  type WavePeer,
} from "../utils/peers";
import { formatInvokeError } from "../utils/errors";

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
 * Settings: other Waves on the network. Paired ones show whether they are
 * in reach and what they may see, the rest can be asked to share.
 */
export default function ConnectedDevices({
  peers,
  onChanged,
  onChoose,
  setError,
}: {
  peers: WavePeer[];
  onChanged: () => void;
  onChoose: (peer: PeerEvent, answering: boolean) => void;
  setError: (message: string | null) => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);

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

  return (
    <div className="wave-devices mset-nested-row">
      <p className="wave-devices-title">Wave devices</p>
      {peers.length === 0 ? (
        <p className="wave-devices-empty">
          Other Waves on this network show up here when Outside Sourcing is on
          for them too.
        </p>
      ) : (
        <ul className="wave-devices-list">
          {peers.map((peer) => (
            <li key={peer.id} className="wave-device">
              <BiDevices className="wave-device-icon" aria-hidden />
              <span className="wave-device-text">
                <span className="wave-device-name">
                  <span
                    className={`wave-device-dot${peer.online ? " online" : ""}`}
                    title={peer.online ? "Available" : "Offline"}
                    aria-label={peer.online ? "Available" : "Offline"}
                  />
                  {peer.name}
                  {peer.playing_from_me && (
                    <span className="wave-device-live">Playing from you</span>
                  )}
                </span>
                <span className="wave-device-hint">
                  {peer.paired
                    ? `${peer.online ? "Available" : "Offline"} · ${shareLine(peer)}`
                    : peer.asking
                      ? "Wants to share libraries"
                      : peer.asked
                        ? "Waiting for it to answer…"
                        : "On your network"}
                </span>
              </span>
              <span className="wave-device-actions">
                {peer.paired && (
                  <>
                    <button
                      className="btn-ghost btn-sm"
                      type="button"
                      disabled={busy === peer.id}
                      onClick={() => onChoose(peer, false)}
                    >
                      Choose songs
                    </button>
                    {peer.share !== "everything" && (
                      <button
                        className="btn-ghost btn-sm"
                        type="button"
                        disabled={busy === peer.id}
                        onClick={() =>
                          void run(
                            peer.id,
                            () => setWaveShare(peer.id, { kind: "everything" }),
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
                  </>
                )}
                {!peer.paired && peer.asking && (
                  <>
                    <button
                      className="btn-ghost btn-sm"
                      type="button"
                      disabled={busy === peer.id}
                      onClick={() => onChoose(peer, true)}
                    >
                      Choose…
                    </button>
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
                  </>
                )}
                {!peer.paired && !peer.asking && !peer.asked && (
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
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
