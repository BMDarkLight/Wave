#!/bin/sh
# Wave — Copyright (C) 2025 BMDarkLight
# Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
# See the LICENSE file in the project root for the full license text.
# https://github.com/BMDarkLight/Wave
#
# Links the `wave` command from an installed Wave.app onto your PATH.
#
# A .dmg is a drag-and-drop image and cannot run install scripts, so on macOS
# the CLI symlink is a separate opt-in step. `brew install --cask wave` does
# this for you; run this script if you installed the .dmg by hand.
#
#   Install:    ./install-cli.sh
#   Uninstall:  ./install-cli.sh --uninstall

set -eu

APP="${WAVE_APP:-/Applications/Wave.app}"
TARGET="$APP/Contents/MacOS/wave"

# /usr/local/bin is on the default PATH but needs sudo on most systems;
# ~/.local/bin needs no privileges but is not always on PATH.
if [ -w /usr/local/bin ] || [ "$(id -u)" = "0" ]; then
    BIN_DIR="/usr/local/bin"
else
    BIN_DIR="$HOME/.local/bin"
fi
BIN_DIR="${WAVE_BIN_DIR:-$BIN_DIR}"
LINK="$BIN_DIR/wave"

if [ "${1:-}" = "--uninstall" ]; then
    if [ -L "$LINK" ]; then
        rm -f "$LINK"
        echo "Removed $LINK"
    else
        echo "No Wave symlink at $LINK; nothing to do."
    fi
    exit 0
fi

if [ ! -x "$TARGET" ]; then
    echo "Wave is not installed at $APP." >&2
    echo "Move Wave.app to /Applications, or set WAVE_APP to its location." >&2
    exit 1
fi

mkdir -p "$BIN_DIR"

# Only ever replace a symlink. A real file there belongs to something else and
# is not ours to delete.
if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then
    echo "$LINK already exists and is not a symlink; refusing to replace it." >&2
    exit 1
fi

ln -sf "$TARGET" "$LINK"
echo "Linked $LINK -> $TARGET"

case ":${PATH}:" in
    *":$BIN_DIR:"*) echo "Run 'wave --help' to get started." ;;
    *) echo "Note: $BIN_DIR is not on your PATH. Add this to your shell profile:"
       echo "    export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
