#!/bin/bash
# Builds PSF Studio's own disk image, `PSF Studio_<version>_aarch64.dmg`,
# for a Mac whose PSF Guard runs on a server, or that has none: the same
# PSF Studio.app that ships inside PSF Guard.app, with a link to
# Applications.
#
# Run scripts/package-macos.sh first: it builds and signs PSF Studio.app at
# target/release/studio-bundle/PSF Studio.app, which this script reuses so
# both copies are one build. A different app can be given as the first
# argument.
#
# The image is signed with the identity package-macos.sh uses
# (PSF_STUDIO_SIGN_IDENTITY, else APPLE_SIGNING_IDENTITY, else
# tauri.conf.json), and left unsigned for "-". Notarization is the release
# workflow's: it notarizes and staples the app before this runs, and the
# image after. Prints the image's path.

set -euo pipefail

app=${1:-"$PWD/target/release/studio-bundle/PSF Studio.app"}
test -d "$app" || {
    echo "No PSF Studio.app at $app; run scripts/package-macos.sh first" >&2
    exit 1
}

setting() {
    python3 -c "import json, sys; value = json.load(open('tauri.conf.json'))
for key in sys.argv[1:]: value = value[key]
print(value)" "$@"
}
version=$(setting version)
version=${version%%-*}
identity=${PSF_STUDIO_SIGN_IDENTITY:-${APPLE_SIGNING_IDENTITY:-$(setting bundle macOS signingIdentity)}}
case $(uname -m) in
    arm64) arch=aarch64 ;;
    *) arch=$(uname -m) ;;
esac

out="$PWD/target/release/studio-dmg"
mkdir -p "$out"
image="$out/PSF Studio_${version}_${arch}.dmg"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
# ditto keeps the app's signature and its stapled ticket.
ditto "$app" "$stage/PSF Studio.app"
ln -s /Applications "$stage/Applications"
rm -f "$image"
hdiutil create -volname "PSF Studio" -srcfolder "$stage" -format UDZO -quiet "$image"
if [ "$identity" != "-" ]; then
    codesign --force --timestamp --sign "$identity" "$image" >&2
fi
hdiutil verify -quiet "$image"
echo "$image"
