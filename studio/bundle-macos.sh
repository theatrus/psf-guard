#!/bin/sh
# Builds PSF Studio.app from the release build: the psf-studio program, its
# icon (see generate-icons.sh), the
# psf-guard program beside it when one is given, and an Info.plist that
# makes the app the handler of psf-studio:// links.
#
# Usage: studio/bundle-macos.sh [PSF_GUARD_PROGRAM] [OUTPUT_FOLDER]
#
# PSF_STUDIO_VERSION sets the app's version (default 0.0.0). With
# PSF_STUDIO_SIGN_IDENTITY the app is signed for the hardened runtime with a
# secure timestamp, as notarization asks of code inside PSF Guard.app; "-"
# signs it ad hoc for a local build.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
psf_guard=${1:-}
out=${2:-"${CARGO_TARGET_DIR:-$here/target}/release"}
app="$out/PSF Studio.app"

(cd "$here" && cargo build --release -p psf-studio)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$here/icons/icon.icns" "$app/Contents/Resources/psf-studio.icns"
target="${CARGO_TARGET_DIR:-$here/target}"
cp "$target/release/psf-studio" "$app/Contents/MacOS/psf-studio"
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
<key>CFBundleDisplayName</key><string>PSF Studio</string>
<key>CFBundleIconFile</key><string>psf-studio</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>__VERSION__</string>
<key>CFBundleVersion</key><string>__VERSION__</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleURLTypes</key>
<array><dict>
<key>CFBundleURLName</key><string>com.psfguard.studio.open</string>
<key>CFBundleURLSchemes</key><array><string>psf-studio</string></array>
</dict></array>
</dict></plist>
PLIST
# The app must run on a Mac with nothing else installed, so Studio may link
# only the system's own libraries. Homebrew's would load on this computer
# and fail on another one.
outside=$(otool -L "$app/Contents/MacOS/psf-studio" | tail -n +2 |
    awk '{print $1}' | grep -v -e '^/System/' -e '^/usr/lib/' || true)
if [ -n "$outside" ]; then
    echo "PSF Studio links libraries macOS does not include:" >&2
    echo "$outside" >&2
    exit 1
fi
version=${PSF_STUDIO_VERSION:-0.0.0}
sed -i '' "s/__VERSION__/$version/g" "$app/Contents/Info.plist"
identity=${PSF_STUDIO_SIGN_IDENTITY:-}
if [ -n "$identity" ]; then
    if [ "$identity" = "-" ]; then
        codesign --force --sign - "$app" >&2
    else
        codesign --force --options runtime --timestamp \
            --entitlements "$here/Entitlements.plist" \
            --sign "$identity" "$app" >&2
    fi
    codesign --verify --strict --verbose=1 "$app" >&2
fi
echo "$app"
