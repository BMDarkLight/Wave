# Security Policy

Wave is an offline-first music player. Out of the box it opens no ports and looks for no other devices. It only makes two kinds of outgoing requests, both of which you can turn off in Settings: it fetches synced lyrics from [LRCLIB](https://lrclib.net) and checks this repository for updates.

Everything else applies only once **Outside Sourcing** is turned on, which is when Wave starts talking to your own media servers and to other copies of Wave on your local network.

For network communication between Waves, Wave uses [Radii](https://github.com/BMDarkLight/Radii). Radii's own [security policy](https://github.com/BMDarkLight/Radii/blob/v0.1.0/SECURITY.md) covers the parts of it Wave relies on, and its precautions apply to Wave as well. This document covers what Wave adds on top.

## Supported versions

| Version                | Supported                    |
| ---------------------- | ---------------------------- |
| Latest release (1.2.x) | Security fixes               |
| Older releases         | Update to the latest release |

Desktop copies of Wave offer updates on their own. Android updates come from the APKs on the releases page.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

- Use **GitHub Security Advisories**: _Security_ → _Report a vulnerability_ on this repository.
- If that isn't available to you, contact the maintainer through the GitHub profile linked from this repository.

Please include the Wave version, your platform, what an attacker needs (same device, same local network, or the internet), and steps to reproduce. A problem in Radii itself is best reported to [Radii](https://github.com/BMDarkLight/Radii/security); if you aren't sure which side it is on, report it here and it will be passed along.

You can expect an acknowledgement within a few days. Fixes follow coordinated disclosure: please allow a reasonable window, typically 90 days, before discussing an issue publicly, unless it is already being exploited.

## What Wave exposes, and to whom

With Outside Sourcing on, Wave opens two things on your machine:

| What                                                             | Port                        | Who can reach it              |
| ---------------------------------------------------------------- | --------------------------- | ----------------------------- |
| The Radii LAN beacon, which announces this Wave and hears others | UDP 7101                    | Devices on your local network |
| Wave's own API, for pairing, browsing and playing shared songs   | TCP 50800, or any free port | Devices on your local network |

**Nothing is reachable from the internet.** Both refuse anything that doesn't come from a loopback, private (RFC 1918) or link-local address, before reading it:

- The beacon is Radii's. It drops datagrams from outside the local network, answers each host at most once a second and no more than 64 hosts a second in all, so it can't be turned into a traffic reflector. See the [LAN beacon section](https://github.com/BMDarkLight/Radii/blob/v0.1.0/SECURITY.md#lan-beacon) of Radii's policy.
- Wave's API refuses connections from any other address as soon as they are accepted.

Neither is a reason to forward these ports on your router, and Wave never asks you to.

## How Waves trust each other

- **Finding is not trusting.** A beacon carries no proof of who sent it, by design, so a device that announces itself is only a hint. Wave asks it who it is over its API before listing it, and lists it as a device you have not paired with.
- **Pairing needs both owners.** A Wave can ask to pair, but nothing is shared until the owner of the other device accepts, and each side then picks what the other may see: everything, nothing, or chosen songs.
- **Paired requests carry a secret.** Pairing gives the two Waves a random secret of 244 bits, which every later request must present. Wave compares it in constant time.
- **Casting is limited to the songs played.** Playing on another Wave lets it stream only the songs you send it, even ones you don't share with it, and nothing else.
- Forgetting a device, or deleting its playlist, removes the pairing on both sides.

## Known limitations

These are open, and worth knowing before you use Devices on a network you don't trust:

- **Traffic between Waves is not encrypted.** Wave's API is plain HTTP. Anyone who can watch your local network can see the music that moves between your devices, and can read a pairing's secret and use it to act as that device. Use Devices on networks you trust, such as your home Wi-Fi or your phone's hotspot, not on public or shared Wi-Fi.
- **A device's name is whatever it says.** Another device on your network can announce itself under any name, including one of yours. Check before you accept a pairing request you didn't expect.
- **Anyone on your network can see that Wave is running**, under this device's name. You can rename it on the Devices page.

Radii offers mutual TLS for its own connections, but Wave's API does not use it yet. Moving Wave's traffic onto an authenticated, encrypted channel is the planned fix for the first two points.

## Outside sources

Accounts you add for Plex, Jellyfin, Subsonic, WebDAV, DLNA or Kodi are stored in Wave's library database on this device and sent only to the server you entered. Use HTTPS addresses for servers that offer them; Wave connects to whatever address you give it.

## Updates

Desktop updates are downloaded from this repository's GitHub releases over HTTPS, and Wave installs one only if it is signed with the release key built into the app.
