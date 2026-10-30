/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { listenTo, safeInvoke } from "./player";

/** Another Wave on the network, paired or not. */
export interface WavePeer {
  id: string;
  name: string;
  online: boolean;
  paired: boolean;
  /** What this Wave shares with it. */
  share: "everything" | "songs" | "nothing";
  share_count: number;
  /** It asked to share and waits for an answer. */
  asking: boolean;
  /** This Wave asked it and waits for its answer. */
  asked: boolean;
  playlist_id: string | null;
  /** It is playing one of this Wave's songs. */
  playing_from_me: boolean;
}

export type ShareChoice =
  | { kind: "everything" }
  | { kind: "songs"; paths: string[] }
  | { kind: "nothing" };

export interface PeerEvent {
  id: string;
  name: string;
}

export const listWavePeers = (): Promise<WavePeer[]> =>
  safeInvoke<WavePeer[]>("list_wave_peers");

export const connectWavePeer = (id: string): Promise<void> =>
  safeInvoke("connect_wave_peer", { id });

export const answerWaveRequest = (
  id: string,
  accept: boolean,
  share: ShareChoice,
): Promise<void> => safeInvoke("answer_wave_request", { id, accept, share });

export const setWaveShare = (id: string, share: ShareChoice): Promise<void> =>
  safeInvoke("set_wave_share", { id, share });

export const getWaveSharePaths = (id: string): Promise<string[]> =>
  safeInvoke<string[]>("get_wave_share_paths", { id });

export const dismissWavePeer = (id: string): Promise<void> =>
  safeInvoke("dismiss_wave_peer", { id });

export const forgetWavePeer = (id: string): Promise<void> =>
  safeInvoke("forget_wave_peer", { id });

/** Play on a paired Wave from now on, starting with what is loaded here. */
export const castToWave = (id: string): Promise<void> =>
  safeInvoke("cast_to_wave", { id });

/** Play on this device again, from where the other one was. */
export const stopCasting = (): Promise<void> => safeInvoke("stop_casting");

/** Playing to or from another Wave started, stopped or moved on. */
export const onWaveCastChanged = (handler: () => void) =>
  listenTo<unknown>("wave-cast-changed", () => handler());

/** The Wave being played to went away; playback is back here, paused. */
export const onWaveCastEnded = (handler: (reason: string) => void) =>
  listenTo<{ reason: string }>("wave-cast-ended", (event) =>
    handler(event.reason),
  );

export const onWavePeerFound = (handler: (peer: PeerEvent) => void) =>
  listenTo<PeerEvent>("wave-peer-found", handler);

export const onWavePeerRequest = (handler: (peer: PeerEvent) => void) =>
  listenTo<PeerEvent>("wave-peer-request", handler);

export const onWavePeerAnswered = (handler: (peer: PeerEvent) => void) =>
  listenTo<PeerEvent>("wave-peer-answered", handler);

export const onWavePeerDeclined = (handler: (peer: PeerEvent) => void) =>
  listenTo<PeerEvent>("wave-peer-declined", handler);

export const onWavePeersChanged = (handler: () => void) =>
  listenTo<unknown>("wave-peers-changed", () => handler());
