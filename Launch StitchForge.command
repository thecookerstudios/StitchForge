#!/bin/bash
# Double-click this file in Finder to build (first time only) and open StitchForge.
# Works from any folder, including names with spaces or symbols.

cd "$(dirname "$0")" || exit 1
echo "StitchForge launcher — folder: $(pwd)"
echo

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

if ! command -v cargo >/dev/null 2>&1; then
  if ! xcode-select -p >/dev/null 2>&1; then
    echo "macOS needs its free Command Line Tools first."
    echo "A window will pop up: click Install, wait until it finishes,"
    echo "then double-click this launcher again."
    xcode-select --install
    read -r -p "Press Enter to close this window." _
    exit 0
  fi
  echo "Installing Rust (one time, about 2 minutes)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y || { read -r -p "Rust install failed. Press Enter to close." _; exit 1; }
  source "$HOME/.cargo/env"
fi

# newer StitchForge versions need a recent Rust; this is quick when already up to date
if command -v rustup >/dev/null 2>&1; then
  echo "Checking Rust is up to date..."
  rustup update stable >/dev/null 2>&1 || true
fi

echo "Building StitchForge... (first build takes a few minutes; later ones take seconds)"
if ! cargo build --release; then
  echo
  echo "Build failed. Copy the red error text above and send it to Claude."
  read -r -p "Press Enter to close." _
  exit 1
fi

APP="StitchForge.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/stitchforge-app "$APP/Contents/MacOS/StitchForge"
cp target/release/stitchforge "$APP/Contents/MacOS/stitchforge-cli"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>StitchForge</string>
  <key>CFBundleDisplayName</key><string>StitchForge</string>
  <key>CFBundleIdentifier</key><string>com.thecookerstudios.stitchforge</string>
  <key>CFBundleExecutable</key><string>StitchForge</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.14</string>
  <key>CFBundleVersion</key><string>14</string>
  <key>LSMinimumSystemVersion</key><string>10.13</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo
echo "Done. Opening StitchForge. Next time you can just double-click StitchForge.app in this folder (or drag it to your Dock)."
open "$APP"
