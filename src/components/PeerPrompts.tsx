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
import { BiDevices, BiX } from "react-icons/bi";
import type { PeerPrompt } from "../hooks/useWavePeers";
import {
  answerWaveRequest,
  connectWavePeer,
  dismissWavePeer,
  setWaveShare,
  type PeerEvent,
} from "../utils/peers";
import { formatInvokeError } from "../utils/errors";

/**
 * What Wave asks about other Waves, one at a time: one was found on the
 * network, one wants to share, or one agreed and this side picks what it
 * shares back. "Choose..." hands over to the song picker.
 */
export default function PeerPrompts({
  prompt,
  onSettled,
  onChoose,
  setError,
}: {
  prompt: PeerPrompt | null;
  onSettled: (id: string) => void;
  /** Open the song picker to choose what `peer` sees; `answering` is true
   *  while its request still waits for an answer. */
  onChoose: (peer: PeerEvent, answering: boolean) => void;
  setError: (message: string | null) => void;
}) {
  const [busy, setBusy] = useState(false);
  if (!prompt) return null;
  const { peer } = prompt;

  const run = async (work: () => Promise<void>, failure: string) => {
    setBusy(true);
    try {
      await work();
      onSettled(peer.id);
    } catch (err) {
      setError(formatInvokeError(err, failure));
    } finally {
      setBusy(false);
    }
  };

  if (prompt.kind === "found" || prompt.kind === "declined") {
    const found = prompt.kind === "found";
    return (
      <div className="update-toast peer-toast" role="status" aria-live="polite">
        <BiDevices className="peer-toast-icon" aria-hidden />
        <span className="update-toast-text">
          {found
            ? `${peer.name} is on your network`
            : `${peer.name} didn't share this time`}
        </span>
        {found && (
          <button
            className="btn-primary"
            type="button"
            disabled={busy}
            onClick={() =>
              void run(
                () => connectWavePeer(peer.id),
                `Couldn't reach ${peer.name}`,
              )
            }
          >
            Share libraries
          </button>
        )}
        <button
          className="update-toast-close"
          type="button"
          aria-label="Not now"
          title="Not now"
          onClick={() => {
            if (found) void dismissWavePeer(peer.id).catch(() => {});
            onSettled(peer.id);
          }}
        >
          <BiX />
        </button>
      </div>
    );
  }

  const answering = prompt.kind === "request";
  const share = (everything: boolean) =>
    run(
      () =>
        answering
          ? answerWaveRequest(
              peer.id,
              true,
              everything ? { kind: "everything" } : { kind: "nothing" },
            )
          : setWaveShare(
              peer.id,
              everything ? { kind: "everything" } : { kind: "nothing" },
            ),
      `Couldn't answer ${peer.name}`,
    );

  return (
    <div className="modal-backdrop">
      <div
        className="modal-dialog confirm-dialog peer-dialog"
        role="dialog"
        aria-label={answering ? `${peer.name} wants to share` : "Share back"}
      >
        <div className="peer-dialog-icon" aria-hidden>
          <BiDevices />
        </div>
        <div className="modal-header">
          <h2>
            {answering
              ? `${peer.name} wants to share libraries`
              : `${peer.name} is sharing with you`}
          </h2>
        </div>
        <p className="confirm-text">
          {answering
            ? "Its music shows up here, and it can play what you share with it. You can change this later in Settings."
            : "What do you share back? You can change this later in Settings."}
        </p>
        <div className="modal-actions peer-actions">
          <button
            className="btn-ghost"
            type="button"
            disabled={busy}
            onClick={() =>
              answering
                ? void run(
                    () =>
                      answerWaveRequest(peer.id, false, { kind: "nothing" }),
                    `Couldn't answer ${peer.name}`,
                  )
                : void share(false)
            }
          >
            {answering ? "Not now" : "Nothing"}
          </button>
          <button
            className="btn-ghost"
            type="button"
            disabled={busy}
            onClick={() => {
              onSettled(peer.id);
              onChoose(peer, answering);
            }}
          >
            Choose…
          </button>
          <button
            className="btn-primary"
            type="button"
            disabled={busy}
            onClick={() => void share(true)}
          >
            Share everything
          </button>
        </div>
      </div>
    </div>
  );
}
