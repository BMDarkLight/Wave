# Installing Wave

Wave ships as a normal desktop application on every platform: it lands in your
app menu with an icon, and it puts a `wave` command on your `PATH` so the
[CLI](../README.md) works from any terminal.

One executable serves both roles — run it with no arguments and the app window
opens, run it with arguments and it behaves as a command-line tool:

```bash
wave --help
wave tracks import ~/Music
wave playback play
```

---

## Linux

### Debian / Ubuntu / Mint

```bash
sudo apt install ./Wave_0.5.0_amd64.deb
```

### Fedora / RHEL / openSUSE

```bash
sudo dnf install ./Wave-0.5.0-1.x86_64.rpm
```

Both packages install the binary to `/usr/bin/wave`, so the command is on
`PATH` immediately, along with a desktop entry and icons.

### AppImage

```bash
chmod +x Wave_0.5.0_amd64.AppImage
./Wave_0.5.0_amd64.AppImage
```

An AppImage is a single self-contained file and deliberately does not install
anything, so it cannot put `wave` on your `PATH` for you. To get the command,
link it somewhere already on `PATH`:

```bash
mkdir -p ~/.local/bin
ln -sf "$PWD/Wave_0.5.0_amd64.AppImage" ~/.local/bin/wave
```

### Arch Linux

Build from source, or install the prebuilt release:

```bash
# from the AUR, once published
yay -S wave-music-player       # builds from source
yay -S wave-music-player-bin   # repackages the official release
```

The `PKGBUILD`s live in [`packaging/arch/`](../packaging/arch).

### Flatpak

```bash
flatpak install app.bmdarklight.wave
flatpak run app.bmdarklight.wave
```

Flatpak applications are sandboxed and are not on your `PATH` under their own
name. For the `wave` command, add a shell alias:

```bash
echo "alias wave='flatpak run app.bmdarklight.wave'" >> ~/.bashrc
```

Building the Flatpak locally requires pinning the dependency trees first,
because Flatpak builds have no network access — see
[`packaging/flatpak/`](../packaging/flatpak).

---

## macOS

### Homebrew (recommended)

```bash
brew install --cask wave
```

This installs `Wave.app` and links the `wave` command onto your `PATH` in one
step.

### DMG

Open the `.dmg` and drag Wave to Applications.

A `.dmg` is a disk image, not an installer, so it cannot run any setup steps —
the `wave` command is a separate opt-in:

```bash
/Applications/Wave.app/Contents/MacOS/wave --help          # works right away
./packaging/macos/install-cli.sh                           # adds it to PATH
```

`install-cli.sh` symlinks the binary into `/usr/local/bin` (or `~/.local/bin`
when that is not writable). Undo it with `install-cli.sh --uninstall`.

---

## Windows

### winget

```powershell
winget install BMDarkLight.Wave
```

### Chocolatey

```powershell
choco install wave
```

### Installer

Run `Wave_0.5.0_x64-setup.exe` and follow the prompts. You can install for
just yourself or for all users.

The installer adds its `bin` directory to `PATH`, so **open a new terminal**
after installing — an already-running shell keeps the `PATH` it started with.

```powershell
wave --help
```

Uninstalling removes the `PATH` entry again.

### Notes for Windows

Wave is a GUI application, which on Windows means it starts with no console
attached. It borrows the console of the terminal that launched it whenever you
pass arguments, so CLI output, piping and redirection all work as expected:

```powershell
wave tracks list | Select-Object -First 10
wave tracks list > tracks.txt
```

`PATH` points at a `bin` directory holding `wave.cmd`, a small shim, rather
than directly at `wave.exe`. Without it an interactive prompt would not wait
for the app to finish and would draw the next prompt over its output.

One consequence: in **PowerShell**, an argument that contains `&` or `|` *and
no spaces* gets split by the shim. Arguments containing spaces are unaffected,
so this is rare in practice:

```powershell
wave tracks query "Simon & Garfunkel"   # fine — contains spaces
wave tracks query "AC&DC"               # split by the shim
wave.exe tracks query "AC&DC"           # workaround: call the exe directly
```

Running from `cmd.exe` is not affected at all.

---

## Verifying an install

```bash
wave --version
```

If the command is not found, the most likely cause is a terminal that was
already open before installing. Open a new one and try again.

---

## Packaging sources

Everything used to produce these packages lives in
[`packaging/`](../packaging):

| Path                   | What it covers                                           |
| ---------------------- | -------------------------------------------------------- |
| `packaging/linux/`     | Desktop entry template and AppStream metainfo            |
| `packaging/windows/`   | NSIS installer hooks, the `PATH` shim and its helper     |
| `packaging/macos/`     | CLI symlink helper for `.dmg` installs                   |
| `packaging/arch/`      | `PKGBUILD` (source) and `PKGBUILD-bin` (release)         |
| `packaging/flatpak/`   | Flatpak manifest and offline source generation           |
| `packaging/homebrew/`  | Homebrew cask                                            |
| `packaging/winget/`    | winget manifests                                         |
| `packaging/chocolatey/`| Chocolatey package                                       |

All installers are built by
[`.github/workflows/build.yml`](../.github/workflows/build.yml) on every push,
and published to a draft GitHub Release when a `v*` tag is pushed. The workflow
also prints SHA-256 checksums for each artifact, which is what the Homebrew,
winget and Chocolatey manifests need before they can be submitted.
