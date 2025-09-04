#!/bin/bash

set -euo pipefail

if [ -f $HOME/.applekeys/use ]; then
    source $HOME/.applekeys/use
fi

bash scripts/build-macos.sh tauri build --no-bundle
scripts/macos-opencv-dylibs.py
bash scripts/build-macos.sh tauri bundle

