# Wave — Homebrew cask
# Copyright (C) 2025 BMDarkLight — AGPL-3.0-or-later
#
# Submit to homebrew/cask, or serve from a tap:
#   brew tap bmdarklight/wave https://github.com/BMDarkLight/homebrew-wave
#   brew install --cask wave
#
# The sha256 values below are placeholders. Fill them from the checksums the
# release workflow prints for the two .dmg assets before publishing.

cask "wave" do
  version "0.5.0"

  on_arm do
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    url "https://github.com/BMDarkLight/Wave/releases/download/v#{version}/Wave_#{version}_aarch64.dmg",
        verified: "github.com/BMDarkLight/Wave/"
  end

  on_intel do
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    url "https://github.com/BMDarkLight/Wave/releases/download/v#{version}/Wave_#{version}_x64.dmg",
        verified: "github.com/BMDarkLight/Wave/"
  end

  name "Wave"
  desc "Lightweight offline-first music player"
  homepage "https://github.com/BMDarkLight/Wave"

  livecheck do
    url :url
    strategy :github_latest
  end

  app "Wave.app"

  # Puts the `wave` command on PATH. The same executable serves both roles:
  # no arguments opens the app, arguments run the CLI.
  binary "#{appdir}/Wave.app/Contents/MacOS/wave"

  zap trash: [
    "~/Library/Application Support/app.bmdarklight.wave",
    "~/Library/Caches/app.bmdarklight.wave",
    "~/Library/Preferences/app.bmdarklight.wave.plist",
    "~/Library/Saved Application State/app.bmdarklight.wave.savedState",
    "~/Library/WebKit/app.bmdarklight.wave",
  ]
end
