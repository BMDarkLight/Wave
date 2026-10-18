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
  jellyfinAddSource,
  jellyfinSignIn,
  setSourcePassword,
  subsonicAddSource,
  type PlexLibrary,
  type PlexServer,
} from "../../utils/remote";

/** The kinds of server a source can be added from. */
type Kind = "plex_tv" | "plex_direct" | "jellyfin" | "subsonic";

type Step =
  | "kind"
  | "sign-in"
  | "form"
  | "token"
  | "password"
  | "server"
  | "libraries"
  | "adding";

const POLL_MS = 2000;

const KINDS: { kind: Kind; label: string }[] = [
  { kind: "plex_tv", label: "Plex" },
  { kind: "plex_direct", label: "Plex server" },
  { kind: "jellyfin", label: "Jellyfin or Emby" },
  {
    kind: "subsonic",
    label: "Navidrome, Airsonic, Gonic and other Subsonic servers",
  },
];

function firstStep(reauthorizing: boolean, method: string): Step {
  if (!reauthorizing) return "kind";
  if (method === "plex_token") return "token";
  if (method === "password") return "password";
  return "sign-in";
}

/**
 * Adds a remote source: a Plex server through plex.tv sign-in or by its
 * address, or a Subsonic-compatible server by address, username and
 * password. With `reauthorizeSourceId` it only signs an existing source in
 * again, the way `reauthorizeMethod` says.
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
  const [step, setStep] = useState<Step>(() =>
    firstStep(reauthorizing, reauthorizeMethod),
  );
  const [kind, setKind] = useState<Kind>("plex_tv");
  const [address, setAddress] = useState("");
  const [username, setUsername] = useState("");
  // The Plex token or the Subsonic password, depending on the kind.
  const [secret, setSecret] = useState("");
  const [jellyfinSession, setJellyfinSession] = useState<string | null>(null);
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

  const finish = () => {
    onDone?.();
    onClose();
  };

  const chooseKind = (next: Kind) => {
    setKind(next);
    setError(null);
    setStep(next === "plex_tv" ? "sign-in" : "form");
  };

  const signedIn = async (session: string) => {
    if (reauthorizing) {
      setStep("adding");
      try {
        await plexReauthorizeSource(reauthorizeSourceId, session);
        finish();
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

  /**
   * Plex and Jellyfin servers show their libraries next; a Subsonic server is
   * added now.
   */
  const submitForm = async () => {
    setError(null);
    if (kind === "subsonic") {
      setStep("adding");
      try {
        await subsonicAddSource(address, username, secret);
        finish();
      } catch (err) {
        fail(err, "Could not add the server");
        setStep("form");
      }
      return;
    }
    setBusy(true);
    try {
      let server;
      if (kind === "jellyfin") {
        const signedIn = await jellyfinSignIn(address, username, secret);
        if (closed.current) return;
        setJellyfinSession(signedIn.session_id);
        server = signedIn;
      } else {
        server = await plexProbeDirect(address, secret);
        if (closed.current) return;
      }
      setLibraries(server.libraries);
      setChosen(new Set(server.libraries.map((library) => library.key)));
      setStep("libraries");
    } catch (err) {
      fail(err, "Could not connect to the server");
    } finally {
      setBusy(false);
    }
  };

  /** Signs an existing source in again with a new token or password. */
  const saveSecret = async () => {
    if (!reauthorizeSourceId) return;
    const back = step;
    setError(null);
    setStep("adding");
    try {
      if (back === "token") {
        await plexSetDirectToken(reauthorizeSourceId, secret);
      } else {
        await setSourcePassword(reauthorizeSourceId, secret);
      }
      finish();
    } catch (err) {
      fail(
        err,
        back === "token"
          ? "Could not use that token"
          : "Could not sign in again",
      );
      setStep(back);
    }
  };

  const add = async () => {
    setError(null);
    setStep("adding");
    try {
      if (kind === "plex_direct") {
        await plexAddDirectSource(address, secret, [...chosen]);
      } else if (kind === "jellyfin" && jellyfinSession) {
        await jellyfinAddSource(jellyfinSession, [...chosen]);
      } else if (sessionId && serverId) {
        await plexAddSource(sessionId, serverId, [...chosen]);
      }
      finish();
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

  const withAccount = kind === "subsonic" || kind === "jellyfin";
  const usesPassword = step === "password" || withAccount;
  const formReady =
    address.trim() !== "" &&
    (!withAccount || (username.trim() !== "" && secret !== ""));
  const secretReady = step === "password" ? secret !== "" : !!secret.trim();
  const secretLabel = usesPassword
    ? "Password"
    : step === "form"
      ? "Token (optional)"
      : "Token";

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
          <h2>{reauthorizing ? "Sign in again" : "Add a remote source"}</h2>
        </div>

        {step === "kind" && (
          <div className="add-source-options">
            {KINDS.map((option) => (
              <button
                key={option.kind}
                className="add-source-option"
                type="button"
                onClick={() => chooseKind(option.kind)}
              >
                {option.label}
              </button>
            ))}
          </div>
        )}

        {(step === "form" || step === "token" || step === "password") && (
          <form
            className="remote-setup-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (step === "form") {
                if (formReady) void submitForm();
              } else if (secretReady) {
                void saveSecret();
              }
            }}
          >
            {step === "form" && (
              <label className="remote-setup-field">
                <span>Server address</span>
                <input
                  type="text"
                  value={address}
                  onChange={(event) => setAddress(event.target.value)}
                  placeholder={
                    kind === "subsonic"
                      ? "192.168.1.10:4533"
                      : kind === "jellyfin"
                        ? "192.168.1.10:8096"
                        : "192.168.1.10:32400"
                  }
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  autoFocus
                />
              </label>
            )}
            {step === "form" && withAccount && (
              <label className="remote-setup-field">
                <span>Username</span>
                <input
                  type="text"
                  value={username}
                  onChange={(event) => setUsername(event.target.value)}
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  autoComplete="username"
                />
              </label>
            )}
            <label className="remote-setup-field">
              <span>{secretLabel}</span>
              <input
                type="password"
                value={secret}
                onChange={(event) => setSecret(event.target.value)}
                placeholder={usesPassword ? undefined : "X-Plex-Token"}
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                autoComplete={usesPassword ? "current-password" : "off"}
                autoFocus={step !== "form"}
              />
            </label>
            {step === "password" ? (
              <p className="mset-hint">
                The password for this account on your server.
              </p>
            ) : kind === "subsonic" ? (
              <p className="mset-hint">
                The account on your server. Wave keeps the password on this
                device to sign in.
              </p>
            ) : kind === "jellyfin" ? (
              <p className="mset-hint">
                The account on your server. Wave signs in once and keeps the
                server's sign-in token, not your password.
              </p>
            ) : (
              <p className="mset-hint">
                No plex.tv needed. Leave the token empty if the server lets your
                network in without signing in (Settings, Network, List of IP
                addresses allowed without auth). Otherwise use the server's
                PlexOnlineToken from its Preferences.xml, or the X-Plex-Token
                shown when you view an item's XML in Plex.
              </p>
            )}
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
          {step === "form" && (
            <button
              className="btn-primary"
              type="button"
              disabled={busy || !formReady}
              onClick={() => void submitForm()}
            >
              {kind === "subsonic"
                ? "Add"
                : kind === "jellyfin"
                  ? "Sign in"
                  : "Connect"}
            </button>
          )}
          {(step === "token" || step === "password") && (
            <button
              className="btn-primary"
              type="button"
              disabled={!secretReady}
              onClick={() => void saveSecret()}
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
