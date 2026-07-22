#!/bin/bash
# Build helper script for macOS (kept for CI compatibility; no special
# environment is needed since the OpenCV dependency was removed).

cargo "$@"
