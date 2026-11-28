#!/bin/bash
# Builds PSF Guard.app and its disk image, with PSF Studio.app inside it at
# Contents/Helpers/PSF Studio.app. Studio ships inside PSF Guard so the
# desktop updater, which replaces the whole app, updates both together.
#
# Studio is signed first, with PSF Guard's identity, for the hardened
# runtime: notarization checks the code inside the app, and the app's own
# signature then seals Studio as it is. PSF_STUDIO_SIGN_IDENTITY overrides
# the identity, such as "-" to sign ad hoc for a local build.
#
# Arguments are passed on to `cargo tauri build`.

set -euo pipefail

if [ -f $HOME/.applekeys/use ]; then
    source $HOME/.applekeys/use
fi

setting() {
    python3 -c "import json, sys; value = json.load(open('tauri.conf.json'))
for key in sys.argv[1:]: value = value[key]
print(value)" "$@"
}

# Apple's version strings are numbers only: 0.15.0-dev ships as 0.15.0.
version=$(setting version)
version=${version%%-*}
identity=${PSF_STUDIO_SIGN_IDENTITY:-${APPLE_SIGNING_IDENTITY:-$(setting bundle macOS signingIdentity)}}

out="$PWD/target/release/studio-bundle"
mkdir -p "$out"
studio=$(PSF_STUDIO_VERSION="$version" PSF_STUDIO_SIGN_IDENTITY="$identity" \
    bash studio/bundle-macos.sh "" "$out" | tail -1)
test -d "$studio"

# Tauri copies the signed Studio into the app's Contents before it signs
# the app itself.
config="$out/psf-studio-in-app.json"
python3 - "$studio" "$config" <<'PY'
import json, sys
studio, config = sys.argv[1:]
json.dump({"bundle": {"macOS": {"files": {"Helpers/PSF Studio.app": studio}}}},
          open(config, "w"), indent=2)
PY

bash scripts/build-macos.sh tauri build --config "$config" "$@"
