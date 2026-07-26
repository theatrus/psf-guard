#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="$(mktemp -d)"
trap 'rm -rf "$output_dir"' EXIT

cd "$repo_root"
cargo tauri icon --output "$output_dir" static/public/psf-guard.svg

for icon in 32x32.png 128x128.png 128x128@2x.png icon.icns icon.ico; do
  cp "$output_dir/$icon" "icons/$icon"
done
