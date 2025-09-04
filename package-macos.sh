#!/bin/bash

set -euo pipefail

bash build-macos.sh tauri build --no-bundle
bash scripts/macos-opencv-dylibs.sh
bash build-macos.sh tauri bundle

