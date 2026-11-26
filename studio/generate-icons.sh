#!/usr/bin/env bash
# Generates PSF Studio's application icons from icons/psf-studio.svg with
# Tauri's generator, as PSF Guard's scripts/generate-icons.sh does for its
# own mark. Do not edit the generated files by hand.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
output_dir="$(mktemp -d)"
trap 'rm -rf "$output_dir"' EXIT

cargo tauri icon --output "$output_dir" "$here/icons/psf-studio.svg" >/dev/null
for icon in 32x32.png 128x128.png 128x128@2x.png icon.icns icon.ico; do
  cp "$output_dir/$icon" "$here/icons/$icon"
done
