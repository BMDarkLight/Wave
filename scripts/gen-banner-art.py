#!/usr/bin/env python3
"""Generate the Wave CLI banner art from the logo's curve.

The mark in assets/app-icon.svg is a thick ribbon tracing a little over one
sine period: the left end sits high, the trough falls at about one quarter of
the width, the crest rises at about two thirds, and the right end sits low.
This samples that curve and renders it with half-block characters, so the
ribbon keeps a constant perpendicular thickness the way the logo does.

Run it from the repo root and paste the output into
src-tauri/src/cli/banner.rs. The art is a compile-time constant there;
nothing computes a sine at startup.

    python3 scripts/gen-banner-art.py
"""

import math

# Sampled to match assets/app-icon.svg. Do not change these without
# re-checking the result against the icon.
PERIODS = 1.15
PHASE = -0.45


def ribbon(width, height, thickness, charset="block"):
    """Render the ribbon into `height` text rows of `width` columns.

    Each text row holds two sub-rows, so vertical resolution is 2 * height.
    """
    sub = height * 2
    mid = (sub - 1) / 2.0
    amp = sub / 2.0 - thickness / 2.0

    grid = [[0] * width for _ in range(sub)]
    for x in range(width):
        t = x / (width - 1)
        angle = 2 * math.pi * PERIODS * t + PHASE
        y = mid + amp * math.sin(angle)
        # A constant perpendicular thickness covers more vertical space where
        # the curve is steep, which is what keeps the ribbon from pinching.
        slope = amp * 2 * math.pi * PERIODS * math.cos(angle) / (width - 1)
        local = thickness * math.sqrt(1 + slope * slope)
        for sy in range(sub):
            if y - local / 2 <= sy <= y + local / 2:
                grid[sy][x] = 1

    # The ASCII ink deliberately avoids the double quote. This art ends up in
    # a Rust source file, and a row like `""#######"""` would force an
    # eight-hash raw string to quote safely.
    full, upper, lower = {
        "block": ("█", "▀", "▄"),
        "ascii": ("#", "'", "_"),
    }[charset]

    rows = []
    for r in range(height):
        up, down = grid[2 * r], grid[2 * r + 1]
        rows.append(
            "".join(
                full if up[x] and down[x] else upper if up[x] else lower if down[x] else " "
                for x in range(width)
            ).rstrip()
        )
    return rows


TIERS = [
    ("WIDE", dict(width=52, height=6, thickness=2.4)),
    ("COMPACT", dict(width=30, height=4, thickness=1.9)),
    ("WIDE_ASCII", dict(width=52, height=6, thickness=2.4, charset="ascii")),
    ("COMPACT_ASCII", dict(width=30, height=4, thickness=1.9, charset="ascii")),
]


def main():
    for name, args in TIERS:
        rows = ribbon(**args)
        print(f"// {name}, {max(len(r) for r in rows)} columns wide")
        for row in rows:
            print(f"    {rust_raw(row)},")
        print()


def rust_raw(text):
    """Wrap `text` as a Rust raw string, using the fewest hashes that are safe.

    A bare r"..." needs the row to hold no double quote at all. An r#"..."# with
    n hashes needs the row to hold no double quote followed by n or more
    hashes, which is what would close the literal early.
    """
    hashes = 0
    while ('"' in text) if hashes == 0 else (('"' + "#" * hashes) in text):
        hashes += 1
        if hashes > 8:
            raise ValueError(f"cannot quote row: {text!r}")
    h = "#" * hashes
    return f'r{h}"{text}"{h}'


if __name__ == "__main__":
    main()
