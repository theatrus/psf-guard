#!/bin/sh
# Builds PSF Studio.app from the release build: the psf-studio program, the
# psf-guard program beside it when one is given, and an Info.plist that
# makes the app the handler of psf-studio:// links.
#
# Usage: studio/bundle-macos.sh [PSF_GUARD_PROGRAM] [OUTPUT_FOLDER]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
psf_guard=${1:-}
out=${2:-"$here/target/release"}
app="$out/PSF Studio.app"

(cd "$here" && cargo build --release -p psf-studio)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$here/target/release/psf-studio" "$app/Contents/MacOS/psf-studio"
if [ -n "$psf_guard" ]; then
    # Studio looks for psf-guard beside itself first.
    cp "$psf_guard" "$app/Contents/MacOS/psf-guard"
fi
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>psf-studio</string>
<key>CFBundleIdentifier</key><string>com.psfguard.studio</string>
<key>CFBundleName</key><string>PSF Studio</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleURLTypes</key>
<array><dict>
<key>CFBundleURLName</key><string>com.psfguard.studio.open</string>
<key>CFBundleURLSchemes</key><array><string>psf-studio</string></array>
</dict></array>
</dict></plist>
PLIST
echo "$app"
