#!/usr/bin/env bash
# Wave — Copyright (C) 2025 BMDarkLight — AGPL-3.0-or-later
#
# Pins the cargo and npm dependency trees to on-disk sources for the Flatpak
# build, which runs with no network access.
#
# Writes cargo-sources.json and node-sources.json next to this script. Re-run
# after any change to src-tauri/Cargo.lock or package-lock.json.
#
# Requires flatpak-builder-tools:
#   git clone https://github.com/flatpak/flatpak-builder-tools

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TOOLS="${FLATPAK_BUILDER_TOOLS:-}"

if [ -z "$TOOLS" ] || [ ! -d "$TOOLS" ]; then
    cat >&2 <<'MSG'
Set FLATPAK_BUILDER_TOOLS to a checkout of flatpak-builder-tools:

    git clone https://github.com/flatpak/flatpak-builder-tools /tmp/fbt
    FLATPAK_BUILDER_TOOLS=/tmp/fbt ./packaging/flatpak/generate-sources.sh
MSG
    exit 1
fi

echo "==> Generating cargo-sources.json from src-tauri/Cargo.lock"
python3 "$TOOLS/cargo/flatpak-cargo-generator.py" \
    "$REPO/src-tauri/Cargo.lock" \
    -o "$HERE/cargo-sources.json"

echo "==> Generating node-sources.json from package-lock.json"
# The generator reads the lockfile from the directory it is pointed at.
(cd "$REPO" && "$TOOLS/node/flatpak-node-generator.py" npm package-lock.json \
    -o "$HERE/node-sources.json")

echo
echo "Wrote:"
echo "  $HERE/cargo-sources.json"
echo "  $HERE/node-sources.json"
echo
echo "Now build with:"
echo "  flatpak-builder --user --install --force-clean build packaging/flatpak/app.bmdarklight.wave.yml"
