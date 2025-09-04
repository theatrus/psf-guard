#!/bin/bash

set -euo pipefail

if [ -f $HOME/.applekeys/use ]; then
    source $HOME/.applekeys/use
fi

#bash build-macos.sh tauri build --no-bundle
#bash scripts/macos-opencv-dylibs.sh
bash build-macos.sh tauri bundle

