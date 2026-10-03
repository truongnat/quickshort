# Homebrew cask for the unsigned Quickshort build.
# Publish in a custom tap repo `truongnat/homebrew-tap` at Casks/quickshort.rb;
# the release workflow can bump version+sha256 automatically via TAP_TOKEN.
#
#   brew tap truongnat/tap
#   brew install --cask quickshort
cask "quickshort" do
  version "0.1.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/truongnat/quickshort/releases/download/v#{version}/Quickshort-v#{version}-macos.zip"
  name "Quickshort"
  desc "Region screenshot tool with annotations and OCR"
  homepage "https://github.com/truongnat/quickshort"

  app "Quickshort.app"

  # unsigned build: strip quarantine so Gatekeeper doesn't block first launch
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Quickshort.app"]
  end

  zap trash: [
    "~/.config/quickshort",
    "~/Library/LaunchAgents/ai.quickshort.plist",
  ]
end
