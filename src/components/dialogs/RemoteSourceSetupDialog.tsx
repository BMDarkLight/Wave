/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState } from "react";
import { formatInvokeError } from "../../utils/errors";
import {
  plexAddDirectSource,
  plexAddSource,
  plexBeginSignIn,
  plexListLibraries,
  plexListServers,
  plexPollSignIn,
  plexProbeDirect,
  plexReauthorizeSource,
  plexSetDirectToken,
  type PlexLibrary,
  type PlexServer,
} from "../../utils/remote";

type Step =
  "kind" | "sign-in" | "direct" | "token" | "server" | "libraries" | "adding";

const POLL_MS = 2000;

/**
 * Adds a remote source: a Plex server through plex.tv sign-in, or by its
 * address and token without plex.tv. Then pick its music libraries. With
 * `reauthorizeSourceId` it only signs an existing source in again, the way
 * `reauthorizeMethod` says.
 */
export default function RemoteSourceSetupDialog({
  reauthorizeSourceId,
  reauthorizeMethod = "plex_tv",
  onClose,
  onDone,
}: {
  reauthorizeSourceId?: string;
  reauthorizeMethod?: string;
  onClose: () => void;
  onDone?: () => void;
}) {
  const reauthorizing = reauthorizeSourceId != null;
  const [step, setStep] = useState<Step>(
    reauthorizing
      ? reauthorizeMethod === "plex_token"
        ? "token"
        : "sign-in"
      : "kind",
  );
  const [address, setAddress] = useState("");
  const [token, setToken] = useState("");
  const [direct, setDirect] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [servers, setServers] = useState<PlexServer[]>([]);
  const [serverId, setServerId] = useState<string | null>(null);
  const [libraries, setLibraries] = useState<PlexLibrary[]>([]);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const closed = useRef(false);

  useEffect(
    () => () => {
      closed.current = true;
    },
    [],
  );

  const fail = (err: unknown, fallback: string) => {
    if (!closed.current) setError(formatInvokeError(err, fallback));
  };

  const signedIn = async (session: string) => {
    if (reauthorizing) {
      setStep("adding");
      try {
        await plexReauthorizeSource(reauthorizeSourceId, session);
        onDone?.();
        onClose();
      } catch (err) {
        fail(err, "Could not sign in again");
        setStep("sign-in");
      }
      return;
    }
    setBusy(true);
    try {
      const found = await plexListServers(session);
      if (closed.current) return;
      setServers(found);
      setServerId(found[0]?.id ?? null);
      setStep("server");
    } catch (err) {
      fail(err, "Could not list your Plex servers");
    } finally {
      setBusy(false);
    }
  };

  // Polls the sign-in while the browser is open.
  useEffect(() => {
    if (!waiting || !sessionId) return;
    let stopped = false;
    const tick = async () => {
      try {
        const state = await plexPollSignIn(sessionId);
        if (stopped) return;
        if (state === "done") {
          setWaiting(false);
          void signedIn(sessionId);
          return;
        }
        if (state === "expired") {
          setWaiting(false);
          setError("The sign-in timed out. Try again.");
          return;
        }
      } catch (err) {
        if (stopped) return;
        setWaiting(false);
        fail(err, "Could not check the sign-in");
        return;
      }
      timer = window.setTimeout(tick, POLL_MS);
    };
    let timer = window.setTimeout(tick, POLL_MS);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
    // signedIn reads the latest props; restarting the poll for it would
    // double the requests.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [waiting, sessionId]);

  const beginSignIn = async () => {
    setError(null);
    setBusy(true);
    try {
      const started = await plexBeginSignIn();
      if (closed.current) return;
      setSessionId(started.session_id);
      setWaiting(true);
    } catch (err) {
      fail(err, "Could not start signing in to Plex");
    } finally {
      setBusy(false);
    }
  };

  const chooseServer = async () => {
    if (!sessionId || !serverId) return;
    setError(null);
    setBusy(true);
    try {
      const found = await plexListLibraries(sessionId, serverId);
      if (closed.current) return;
      setLibraries(found);
      setChosen(new Set(found.map((library) => library.key)));
      setStep("libraries");
    } catch (err) {
      fail(err, "Could not list the server's libraries");
    } finally {
      setBusy(false);
    }
  };

  const connectDirect = async () => {
    setError(null);
    setBusy(true);
    try {
      const server = await plexProbeDirect(address, token);
      if (closed.current) return;
      setDirect(true);
      setLibraries(server.libraries);
      setChosen(new Set(server.libraries.map((library) => library.key)));
      setStep("libraries");
    } catch (err) {
      fail(err, "Could not connect to the server");
    } finally {
      setBusy(false);
    }
  };

  const saveToken = async () => {
    if (!reauthorizeSourceId) return;
    setError(null);
    setStep("adding");
    try {
      await plexSetDirectToken(reauthorizeSourceId, token);
      onDone?.();
      onClose();
    } catch (err) {
      fail(err, "Could not use that token");
      setStep("token");
    }
  };

  const add = async () => {
    if (!direct && (!sessionId || !serverId)) return;
    setError(null);
    setStep("adding");
    try {
      if (direct) {
        await plexAddDirectSource(address, token, [...chosen]);
      } else if (sessionId && serverId) {
        await plexAddSource(sessionId, serverId, [...chosen]);
      }
      onDone?.();
      onClose();
    } catch (err) {
      fail(err, "Could not add the server");
      setStep("libraries");
    }
  };

  const toggleLibrary = (key: string) => {
    setChosen((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog remote-setup-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>
            {reauthorizing ? "Sign in to Plex again" : "Add a remote source"}
          </h2>
        </div>

        {step === "kind" && (
          <div className="add-source-options">
            <button
              className="add-source-option"
              type="button"
              onClick={() => setStep("sign-in")}
            >
              Plex (sign in with plex.tv)
            </button>
            <button
              className="add-source-option"
              type="button"
              onClick={() => setStep("direct")}
            >
              Plex server (connect by address)
            </button>
          </div>
        )}

        {(step === "direct" || step === "token") && (
          <form
            className="remote-setup-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (step === "direct") void connectDirect();
              else void saveToken();
            }}
          >
            {step === "direct" && (
              <label className="remote-setup-field">
                <span>Server address</span>
                <input
                  type="text"
                  value={address}
                  onChange={(event) => setAddress(event.target.value)}
                  placeholder="192.168.1.10:32400"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  autoFocus
                />
              </label>
            )}
            <label className="remote-setup-field">
              <span>{step === "direct" ? "Token (optional)" : "Token"}</span>
              <input
                type="password"
                value={token}
                onChange={(event) => setToken(event.target.value)}
                placeholder="X-Plex-Token"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                autoFocus={step === "token"}
              />
            </label>
            <p className="mset-hint">
              No plex.tv needed. Leave the token empty if the server lets your
              network in without signing in (Settings, Network, List of IP
              addresses allowed without auth). Otherwise use the server's
              PlexOnlineToken from its Preferences.xml, or the X-Plex-Token
              shown when you view an item's XML in Plex.
            </p>
            {/* Submits on Enter; the visible button is in the actions row. */}
            <button type="submit" hidden />
          </form>
        )}

        {step === "sign-in" &&
          (waiting ? (
            <p className="confirm-text">Finish signing in in your browser.</p>
          ) : (
            <div className="add-source-options">
              <button
                className="btn-primary"
                type="button"
                disabled={busy}
                onClick={() => void beginSignIn()}
              >
                Sign in with Plex
              </button>
            </div>
          ))}

        {step === "server" &&
          (servers.length === 0 ? (
            <p className="confirm-text">
              This Plex account has no servers Wave can use.
            </p>
          ) : (
            <div className="remote-setup-list" role="radiogroup">
              {servers.map((server) => (
                <label key={server.id} className="remote-setup-row">
                  <input
                    type="radio"
                    name="plex-server"
                    checked={serverId === server.id}
                    onChange={() => setServerId(server.id)}
                  />
                  <span>{server.name}</span>
                  {!server.owned && (
                    <span className="remote-setup-note">Shared with you</span>
                  )}
                </label>
              ))}
            </div>
          ))}

        {step === "libraries" &&
          (libraries.length === 0 ? (
            <p className="confirm-text">This server has no music libraries.</p>
          ) : (
            <div className="remote-setup-list">
              {libraries.map((library) => (
                <label key={library.key} className="remote-setup-row">
                  <input
                    type="checkbox"
                    checked={chosen.has(library.key)}
                    onChange={() => toggleLibrary(library.key)}
                  />
                  <span>{library.title}</span>
                </label>
              ))}
            </div>
          ))}

        {step === "adding" && (
          <p className="confirm-text">
            {reauthorizing ? "Signing in…" : "Adding the server…"}
          </p>
        )}

        {error && <p className="remote-setup-error">{error}</p>}

        <div className="modal-actions">
          <button className="btn-ghost" onClick={onClose} type="button">
            Cancel
          </button>
          {step === "server" && (
            <button
              className="btn-primary"
              type="button"
              disabled={busy || !serverId}
              onClick={() => void chooseServer()}
            >
              Continue
            </button>
          )}
          {step === "direct" && (
            <button
              className="btn-primary"
              type="button"
              disabled={busy || !address.trim()}
              onClick={() => void connectDirect()}
            >
              Connect
            </button>
          )}
          {step === "token" && (
            <button
              className="btn-primary"
              type="button"
              disabled={!token.trim()}
              onClick={() => void saveToken()}
            >
              Save
            </button>
          )}
          {step === "libraries" && (
            <button
              className="btn-primary"
              type="button"
              disabled={chosen.size === 0}
              onClick={() => void add()}
            >
              Add
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
