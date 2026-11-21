# Security Policy

Wave is an offline-first music player. Out of the box it opens no ports and looks for no other devices. It only makes two kinds of outgoing requests, both of which you can turn off in Settings: it fetches synced lyrics from [LRCLIB](https://lrclib.net) and checks this repository for updates. The one exception is the optional `wave serve` command: starting a hosted server intentionally opens a listener, loopback-only or LAN-restricted by default, or accepting any client if you explicitly opt into `internet` scope.

Everything else applies only once **Outside Sourcing** is turned on, which is when Wave starts talking to your own media servers and to other copies of Wave on your local network.

For network communication between Waves, Wave uses [Radii](https://github.com/BMDarkLight/Radii). Radii's own [security policy](https://github.com/BMDarkLight/Radii/blob/v0.1.0/SECURITY.md) covers the parts of it Wave relies on, and its precautions apply to Wave as well. This document covers what Wave adds on top.

## Supported versions

| Version                | Supported                    |
| ---------------------- | ---------------------------- |
| Latest release (1.3.x) | Security fixes               |
| Older releases         | Update to the latest release |

Desktop copies of Wave offer updates on their own. Android updates come from the APKs on the releases page.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

- Use **GitHub Security Advisories**: _Security_ → _Report a vulnerability_ on this repository.
- If that isn't available to you, contact the maintainer through the GitHub profile linked from this repository.

Please include the Wave version, your platform, what an attacker needs (same device, same local network, or the internet), and steps to reproduce. A problem in Radii itself is best reported to [Radii](https://github.com/BMDarkLight/Radii/security); if you aren't sure which side it is on, report it here and it will be passed along.

You can expect an acknowledgement within a few days. Fixes follow coordinated disclosure: please allow a reasonable window, typically 90 days, before discussing an issue publicly, unless it is already being exploited.

## What Wave exposes, and to whom

With Outside Sourcing on, or a hosted server running, Wave may open these
listeners on your machine:

| What                                                                | Port                        | Who can reach it                                                         |
| ------------------------------------------------------------------- | --------------------------- | ------------------------------------------------------------------------ |
| The Radii LAN beacon, which announces this Wave and hears others    | UDP 7101                    | Local-network devices; LAN Wave servers with the beacon on               |
| Wave's `/wave/` API, for pairing, browsing and playing shared songs | TCP 50800, or any free port | Local loopback/LAN clients by scope; never with `subsonic` or `internet` |
| A hosted Subsonic `/rest/` API for third-party players              | TCP 50800, or any free port | Local loopback/LAN clients by scope; any client with `internet`          |

Starting `wave serve` runs the chosen API listener in a background process on
the port you pick (50800 by default). `--service` selects `wave`, `subsonic` or
both (the default); `--scope` selects who may connect: `loopback` binds only
`127.0.0.1` and accepts only local clients, the default `lan` binds all
interfaces but accepts only loopback, private (RFC 1918) and link-local
clients, and `internet` accepts clients from any address. Internet scope is an
explicit opt-in: it is Subsonic-only, never exposes the Wave peer API, keeps
the beacon off, and rejects an explicit conflicting `--service` or
`--beacon on`. The beacon runs only for Wave service at LAN scope. Subsonic
clients authenticate with a user name and password saved owner-only in
`wave-server.json`; a generated password is shown once on the first start. Set
your own with `--password` if you prefer. `wave serve config` changes saved
settings only, which take effect at the next start.

`wave serve` also adds a management listener used by `wave serve stop`, `peers`,
`accept` and `decline`: a socket bound only to `127.0.0.1` that requires a
random token stored owner-only in `wave-server-state.json`. It stays
loopback-only and token-authenticated at every scope, including `internet`.

**By default, the LAN scope refuses non-local clients.** The listener checks
each client's address before reading anything: `loopback` accepts only local
clients, and `lan` refuses anything that does not come from a loopback, private
(RFC 1918) or link-local address. `internet` is the explicit exception: it
accepts any client that can reach the port, but serves Subsonic only, with no
Wave peer API and no beacon.

- The beacon is Radii's. It drops datagrams from outside the local network, answers each host at most once a second and no more than 64 hosts a second in all, so it can't be turned into a traffic reflector. See the [LAN beacon section](https://github.com/BMDarkLight/Radii/blob/v0.1.0/SECURITY.md#lan-beacon) of Radii's policy.
- `loopback` scope binds only 127.0.0.1, and both it and `lan` scope refuse clients from other addresses as soon as a connection is accepted. At `internet` scope, the Subsonic listener accepts any client.

Loopback and LAN scopes give you no reason to forward these ports on your
router, and Wave never asks you to. Internet scope is the deliberate exception:
you must arrange your own firewall and port forwarding or reverse proxy. The
address remote clients use depends on your router and port forwarding; Wave
does not discover a public IP. Wave's APIs are plain HTTP, so credentials and
music travel unencrypted. Wave itself does not terminate TLS: if you expose a
server beyond a trusted network, put it behind a reverse proxy that terminates
TLS.

## How Waves trust each other

- **Finding is not trusting.** A beacon carries no proof of who sent it, by design, so a device that announces itself is only a hint. Wave asks it who it is over its API before listing it, and lists it as a device you have not paired with.
- **Pairing normally needs both owners.** In the desktop app, a Wave can ask to pair but nothing is shared until the other owner accepts, and each side then picks what the other may see: everything, nothing, or chosen songs. A headless `wave serve` has no one to ask, so an unrestricted LAN server answers incoming requests itself with its configured share. Use `--share nothing` or `--auto-accept off` to hold requests for `wave serve peers` and `wave serve accept`.
- **Paired requests carry a secret.** Pairing gives the two Waves a random secret of 244 bits, which every later request must present. Wave compares it in constant time.
- **Casting is limited to the songs played.** Playing on another Wave lets it stream only the songs you send it, even ones you don't share with it, and nothing else.
- Forgetting a device, or deleting its playlist, removes the pairing on both sides.

## Known limitations

These are open, and worth knowing before you use Devices on a network you don't trust:

- **Traffic is not encrypted.** Wave's API is plain HTTP. Anyone who can watch your local network can see the music that moves between your devices, and can read a pairing's secret and use it to act as that device. The Subsonic API used by a hosted server is also plain HTTP: its salted MD5 token protects the password in a request, but not the music or the requests themselves. Use these features on networks you trust, such as your home Wi-Fi or your phone's hotspot, not on public or shared Wi-Fi. If you opt into `internet` scope, terminate TLS at a reverse proxy before exposing the server beyond a trusted network; Wave itself does not terminate TLS.
- **`internet` scope accepts any client that reaches the port.** It serves Subsonic only, with no Wave peer API and no beacon, but anyone who reaches it can try credentials. Use a strong password, restrict access with a firewall, and remember that plain HTTP leaves the requests and the music unencrypted.
- **A device's name is whatever it says.** Another device on your network can announce itself under any name, including one of yours. Check before you accept a pairing request you didn't expect. An unrestricted hosted server pairs automatically, so restrict its share (`--share nothing`) on networks you do not fully trust and approve known devices from the CLI.
- **Anyone on your network can see that Wave is running**, under this device's name. You can rename it on the Devices page.

Radii offers mutual TLS for its own connections, but Wave's API does not use it yet. Moving Wave's traffic onto an authenticated, encrypted channel is the planned fix for the encryption and device-identity points above.

## Outside sources

Accounts you add for Plex, Jellyfin, Subsonic, WebDAV, DLNA or Kodi are stored in Wave's library database on this device and sent only to the server you entered. Use HTTPS addresses for servers that offer them; Wave connects to whatever address you give it.

## Updates

Desktop updates are downloaded from this repository's GitHub releases over HTTPS, and Wave installs one only if it is signed with the release key built into the app.
