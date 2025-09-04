#!/bin/bash

set -euo pipefail

# Script to automatically detect OpenCV dylibs and update Tauri configuration
# This script should be run after building the Rust binary but before Tauri bundling

BINARY_PATH="target/release/psf-guard"
TAURI_CONFIG="tauri.macos.conf.json"
TEMP_CONFIG="tauri.macos.conf.json.tmp"

echo "🔍 Detecting OpenCV dylib dependencies for macOS packaging..."

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo "❌ Binary not found at $BINARY_PATH"
    echo "   Make sure to build the release binary first with: cargo build --release --features tauri"
    exit 1
fi

# Check if tauri config exists
if [ ! -f "$TAURI_CONFIG" ]; then
    echo "❌ Tauri macOS config not found at $TAURI_CONFIG"
    exit 1
fi

echo "📋 Analyzing binary dependencies with otool..."

# Extract OpenCV dylib paths using otool
OPENCV_DYLIBS=$(otool -L "$BINARY_PATH" | grep -E 'libopencv.*\.dylib' | awk '{print $1}' | sort -u || true)

if [ -z "$OPENCV_DYLIBS" ]; then
    echo "⚠️  No OpenCV dylibs found in binary"
    echo "   This is normal if OpenCV is statically linked or not used"
    exit 0
fi

echo "✅ Found OpenCV dylibs:"
echo "$OPENCV_DYLIBS" | while read -r dylib; do
    echo "   - $dylib"
done

# Create JSON array of dylib paths
FRAMEWORKS_JSON="["
FIRST=true
while IFS= read -r dylib; do
    if [ "$FIRST" = true ]; then
        FIRST=false
    else
        FRAMEWORKS_JSON="$FRAMEWORKS_JSON,"
    fi
    FRAMEWORKS_JSON="$FRAMEWORKS_JSON\"$dylib\""
done <<< "$OPENCV_DYLIBS"
FRAMEWORKS_JSON="$FRAMEWORKS_JSON]"

echo "🔧 Updating $TAURI_CONFIG with detected dylibs..."

# Use jq to update the frameworks array in the JSON config
if ! command -v jq &> /dev/null; then
    echo "❌ jq is required but not installed"
    echo "   Install with: brew install jq"
    exit 1
fi

# Update the frameworks array in bundle.macOS
jq --argjson frameworks "$FRAMEWORKS_JSON" '.bundle.macOS.frameworks = $frameworks' "$TAURI_CONFIG" > "$TEMP_CONFIG"
mv "$TEMP_CONFIG" "$TAURI_CONFIG"

echo "✅ Updated frameworks in $TAURI_CONFIG"

# Transform dylib paths using install_name_tool to use @executable_path
echo "🔄 Transforming dylib paths to relative references..."

BUNDLE_BINARY="target/release/psf-guard"


if [ -n "$BUNDLE_BINARY" ] && [ -f "$BUNDLE_BINARY" ]; then
    echo "📦 Found bundled binary: $BUNDLE_BINARY"
    
    # Transform each dylib reference to use @executable_path
    while IFS= read -r dylib; do
        dylib_name=$(basename "$dylib")
        echo "   Transforming: $dylib -> @executable_path/../Frameworks/$dylib_name"
        install_name_tool -change "$dylib" "@executable_path/../Frameworks/$dylib_name" "$BUNDLE_BINARY" || true
    done <<< "$OPENCV_DYLIBS"
    
    echo "✅ Transformed dylib references in bundled binary"
else
    echo "⚠️  Bundled binary not found yet - dylib transformation will be handled by Tauri"
    echo "   The frameworks list has been updated in $TAURI_CONFIG for Tauri to handle"
fi

echo "🎉 OpenCV dylib processing complete!"
