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
  audiusAddSource,
  dlnaAddSource,
  dlnaDiscover,
  jellyfinAddSource,
  jellyfinSignIn,
  kodiAddSource,
  setSourcePassword,
  subsonicAddSource,
  webdavAddSource,
  type DlnaServer,
  type PlexLibrary,
  type PlexServer,
} from "../../utils/remote";

/** The kinds of server a source can be added from. */
type Kind =
  | "plex_tv"
  | "plex_direct"
  | "jellyfin"
  | "subsonic"
  | "webdav"
  | "dlna"
  | "kodi"
  | "audius";

/** Kinds added straight from the form, with no library step after it. */
const ADDED_FROM_FORM: Kind[] = ["subsonic", "webdav", "kodi", "audius"];

type Step =
  | "kind"
  | "dlna"
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
  { kind: "webdav", label: "WebDAV folder: Nextcloud, ownCloud or a NAS" },
  { kind: "dlna", label: "DLNA or UPnP media server" },
  { kind: "kodi", label: "Kodi" },
  { kind: "audius", label: "Audius account" },
];

const PLACEHOLDERS: Partial<Record<Kind, string>> = {
  subsonic: "192.168.1.10:4533",
  jellyfin: "192.168.1.10:8096",
  plex_direct: "192.168.1.10:32400",
  webdav: "https://cloud.example.com/remote.php/dav/files/you/Music",
  kodi: "192.168.1.30:8080",
  audius: "@handle",
};

function firstStep(reauthorizing: boolean, method: string): Step {
  if (!reauthorizing) return "kind";
  if (method === "plex_token") return "token";
  if (method === "password") return "password";
  return "sign-in";
}

/**
 * Adds a remote source: a Plex server through plex.tv sign-in or by its
 * address, a Jellyfin, Emby or Subsonic server, a WebDAV folder, a DLNA
 * server found on the network, a Kodi device, or an Audius account. With
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
  const [dlnaServers, setDlnaServers] = useState<DlnaServer[]>([]);
  const [dlnaChoice, setDlnaChoice] = useState<string | null>(null);
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

  /** Looks for media servers on the local network. */
  const findDlnaServers = async () => {
    setError(null);
    setBusy(true);
    try {
      const found = await dlnaDiscover();
      if (closed.current) return;
      setDlnaServers(found);
      setDlnaChoice(found[0]?.location ?? null);
    } catch (err) {
      fail(err, "Couldn't search your network");
    } finally {
      setBusy(false);
    }
  };

  const chooseKind = (next: Kind) => {
    setKind(next);
    setError(null);
    if (next === "dlna") {
      setStep("dlna");
      void findDlnaServers();
      return;
    }
    setStep(next === "plex_tv" ? "sign-in" : "form");
  };

  /** A typed address wins over a server picked from the list. */
  const dlnaTarget = address.trim() || dlnaChoice;

  const addDlna = async () => {
    if (!dlnaTarget) return;
    setError(null);
    setStep("adding");
    try {
      await dlnaAddSource(dlnaTarget);
      finish();
    } catch (err) {
      fail(err, "Couldn't add the server");
      setStep("dlna");
    }
  };

  const signedIn = async (session: string) => {
    if (reauthorizing) {
      setStep("adding");
      try {
        await plexReauthorizeSource(reauthorizeSourceId, session);
        finish();
      } catch (err) {
        fail(err, "Couldn't sign in again");
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
      fail(err, "Couldn't load your Plex servers");
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
          setError("Sign-in timed out. Try again.");
          return;
        }
      } catch (err) {
        if (stopped) return;
        setWaiting(false);
        fail(err, "Couldn't check the sign-in");
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
      fail(err, "Couldn't start the Plex sign-in");
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
      fail(err, "Couldn't load the server's libraries");
    } finally {
      setBusy(false);
    }
  };

  /**
   * Plex and Jellyfin servers show their libraries next; every other kind is
   * added now.
   */
  const submitForm = async () => {
    setError(null);
    if (ADDED_FROM_FORM.includes(kind)) {
      setStep("adding");
      try {
        if (kind === "subsonic") {
          await subsonicAddSource(address, username, secret);
        } else if (kind === "webdav") {
          await webdavAddSource(address, username, secret);
        } else if (kind === "kodi") {
          await kodiAddSource(address, username, secret);
        } else {
          await audiusAddSource(address);
        }
        finish();
      } catch (err) {
        fail(err, "Couldn't add the server");
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
      fail(err, "Couldn't connect to the server");
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
        back === "token" ? "That token didn't work" : "Couldn't sign in again",
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
      fail(err, "Couldn't add the server");
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
  // A WebDAV folder or a Kodi device may not ask for a login at all.
  const optionalAccount = kind === "webdav" || kind === "kodi";
  const showsAccount = withAccount || optionalAccount;
  const showsSecret = kind !== "audius" || step !== "form";
  const usesPassword = step === "password" || showsAccount;
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
                <span>
                  {kind === "audius"
                    ? "Audius handle"
                    : kind === "webdav"
                      ? "Folder address"
                      : "Server address"}
                </span>
                <input
                  type="text"
                  value={address}
                  onChange={(event) => setAddress(event.target.value)}
                  placeholder={PLACEHOLDERS[kind]}
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  autoFocus
                />
              </label>
            )}
            {step === "form" && showsAccount && (
              <label className="remote-setup-field">
                <span>
                  {optionalAccount ? "Username (optional)" : "Username"}
                </span>
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
            {showsSecret && (
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
            )}
            {step === "password" ? (
              <p className="mset-hint">Enter your password for this server.</p>
            ) : kind === "webdav" ? (
              <p className="mset-hint">
                The address of the folder that holds your music. For Nextcloud,
                copy the WebDAV address from Files, Settings, and add the folder
                to the end. Wave saves the password on this device, and only
                sends it over plain http:// to an address on your own network.
              </p>
            ) : kind === "kodi" ? (
              <p className="mset-hint">
                In Kodi, turn on Settings, Services, Control, &quot;Allow remote
                control via HTTP&quot;, and use the username and password set
                there. Wave saves the password on this device.
              </p>
            ) : kind === "audius" ? (
              <p className="mset-hint">
                The account&apos;s uploads, favorites and reposts join your
                library. They&apos;re public, so there&apos;s nothing to sign in
                to.
              </p>
            ) : kind === "subsonic" ? (
              <p className="mset-hint">
                Use your login for this server. Wave saves the password on this
                device.
              </p>
            ) : kind === "jellyfin" ? (
              <p className="mset-hint">
                Use your login for this server. Wave doesn't save your password.
              </p>
            ) : (
              <p className="mset-hint">
                Works without plex.tv. Leave the token blank if your server lets
                your network in without signing in. Otherwise, in Plex open any
                track, choose Get Info, then View XML, and copy the X-Plex-Token
                from the end of the address.
              </p>
            )}
            {/* Submits on Enter; the visible button is in the actions row. */}
            <button type="submit" hidden />
          </form>
        )}

        {step === "dlna" && (
          <form
            className="remote-setup-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (dlnaTarget) void addDlna();
            }}
          >
            {busy ? (
              <p className="confirm-text">
                Looking for media servers on your network…
              </p>
            ) : dlnaServers.length === 0 ? (
              <p className="confirm-text">
                No media servers answered. Check that the server is on and on
                this network, or enter its address.
              </p>
            ) : (
              <div className="remote-setup-list" role="radiogroup">
                {dlnaServers.map((server) => (
                  <label key={server.location} className="remote-setup-row">
                    <input
                      type="radio"
                      name="dlna-server"
                      checked={
                        !address.trim() && dlnaChoice === server.location
                      }
                      onChange={() => {
                        setAddress("");
                        setDlnaChoice(server.location);
                      }}
                    />
                    <span>{server.name}</span>
                  </label>
                ))}
              </div>
            )}
            <label className="remote-setup-field">
              <span>Or enter its address</span>
              <input
                type="text"
                value={address}
                onChange={(event) => setAddress(event.target.value)}
                placeholder="192.168.1.10:8200"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
              />
            </label>
            <button type="submit" hidden />
          </form>
        )}

        {step === "sign-in" &&
          (waiting ? (
            <p className="confirm-text">
              Finish signing in on the page that opened.
            </p>
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
              No servers found on this Plex account.
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
            <p className="confirm-text">No music libraries on this server.</p>
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
              {ADDED_FROM_FORM.includes(kind)
                ? "Add"
                : kind === "jellyfin"
                  ? "Sign in"
                  : "Connect"}
            </button>
          )}
          {step === "dlna" && (
            <>
              <button
                className="btn-ghost"
                type="button"
                disabled={busy}
                onClick={() => void findDlnaServers()}
              >
                Search again
              </button>
              <button
                className="btn-primary"
                type="button"
                disabled={busy || !dlnaTarget}
                onClick={() => void addDlna()}
              >
                Add
              </button>
            </>
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
