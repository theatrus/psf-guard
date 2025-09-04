#!/bin/bash

set -euo pipefail

# Script to recursively detect all homebrew dylib dependencies, copy them locally,
# rewrite their internal references, and update Tauri configuration

BINARY_PATH="target/release/psf-guard"
TAURI_CONFIG="tauri.macos.conf.json"
TEMP_CONFIG="tauri.macos.conf.json.tmp"
FRAMEWORKS_DIR="Frameworks"
HOMEBREW_PREFIX="/opt/homebrew"

echo "🔍 Detecting and processing all homebrew dylib dependencies for macOS packaging..."

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

# Create frameworks directory
mkdir -p "$FRAMEWORKS_DIR"
echo "📁 Created/verified frameworks directory: $FRAMEWORKS_DIR"

# Function to recursively find all homebrew dylib dependencies
find_homebrew_dependencies() {
    local binary="$1"
    local visited_file="$2"
    
    # Get direct dependencies
    local deps
    deps=$(otool -L "$binary" 2>/dev/null | grep -E "^[[:space:]]*$HOMEBREW_PREFIX.*\.dylib" | awk '{print $1}' || true)
    
    if [ -z "$deps" ]; then
        return
    fi
    
    while IFS= read -r dep; do
        # Skip if already visited
        if grep -Fxq "$dep" "$visited_file" 2>/dev/null; then
            continue
        fi
        
        # Add to visited list
        echo "$dep" >> "$visited_file"
        
        # If the dependency exists, recursively find its dependencies
        if [ -f "$dep" ]; then
            echo "   📦 Found: $dep"
            find_homebrew_dependencies "$dep" "$visited_file"
        else
            echo "   ⚠️  Missing: $dep"
        fi
    done <<< "$deps"
}

echo "📋 Recursively analyzing all homebrew dependencies..."

# Create temporary file to track visited dependencies
VISITED_FILE=$(mktemp)
trap "rm -f $VISITED_FILE" EXIT

# Find all dependencies starting from the main binary
find_homebrew_dependencies "$BINARY_PATH" "$VISITED_FILE"

# Read all unique dependencies
if [ ! -s "$VISITED_FILE" ]; then
    echo "⚠️  No homebrew dylibs found in binary"
    echo "   This is normal if OpenCV is statically linked or not used"
    exit 0
fi

ALL_DYLIBS=$(sort -u "$VISITED_FILE")
echo "✅ Found $(echo "$ALL_DYLIBS" | wc -l | tr -d ' ') unique homebrew dylibs"

# Copy all dylibs to frameworks directory
echo "📥 Copying dylibs to $FRAMEWORKS_DIR..."
LOCAL_DYLIBS=""
while IFS= read -r dylib; do
    if [ -f "$dylib" ]; then
        dylib_name=$(basename "$dylib")
        local_path="$FRAMEWORKS_DIR/$dylib_name"
        
        echo "   Copying: $dylib_name"
        cp "$dylib" "$local_path"
        
        # Add to local dylibs list
        if [ -z "$LOCAL_DYLIBS" ]; then
            LOCAL_DYLIBS="$local_path"
        else
            LOCAL_DYLIBS="$LOCAL_DYLIBS"$'\n'"$local_path"
        fi
    fi
done <<< "$ALL_DYLIBS"

echo "✅ Copied all dylibs to local frameworks directory"

# Rewrite internal dylib references to use @loader_path
echo "🔧 Rewriting internal dylib references..."

# First, rewrite the main binary
echo "   📝 Rewriting main binary: $BINARY_PATH"
while IFS= read -r dylib; do
    dylib_name=$(basename "$dylib")
    echo "      $dylib -> @executable_path/../Frameworks/$dylib_name"
    install_name_tool -change "$dylib" "@executable_path/../Frameworks/$dylib_name" "$BINARY_PATH" 2>/dev/null || true
done <<< "$ALL_DYLIBS"

# Then rewrite each copied dylib's internal references
while IFS= read -r local_dylib; do
    if [ -f "$local_dylib" ]; then
        dylib_name=$(basename "$local_dylib")
        echo "   📝 Rewriting dylib: $dylib_name"
        
        # Update the dylib's own ID first
        install_name_tool -id "@loader_path/$dylib_name" "$local_dylib" 2>/dev/null || true
        
        # Update references to other homebrew dylibs in this dylib
        while IFS= read -r dep_dylib; do
            dep_name=$(basename "$dep_dylib")
            if [ "$dep_name" != "$dylib_name" ]; then
                echo "      $dep_dylib -> @loader_path/$dep_name"
                install_name_tool -change "$dep_dylib" "@loader_path/$dep_name" "$local_dylib" 2>/dev/null || true
            fi
        done <<< "$ALL_DYLIBS"
    fi
done <<< "$LOCAL_DYLIBS"

echo "✅ Rewritten all dylib references"

# Create JSON array of local dylib paths for Tauri
echo "📝 Updating Tauri configuration..."
FRAMEWORKS_JSON="["
FIRST=true
while IFS= read -r local_dylib; do
    # Convert to absolute path for Tauri
    abs_path="$(cd "$(dirname "$local_dylib")" && pwd)/$(basename "$local_dylib")"
    
    if [ "$FIRST" = true ]; then
        FIRST=false
    else
        FRAMEWORKS_JSON="$FRAMEWORKS_JSON,"
    fi
    FRAMEWORKS_JSON="$FRAMEWORKS_JSON\"$abs_path\""
done <<< "$LOCAL_DYLIBS"
FRAMEWORKS_JSON="$FRAMEWORKS_JSON]"

# Use jq to update the frameworks array in the JSON config
if ! command -v jq &> /dev/null; then
    echo "❌ jq is required but not installed"
    echo "   Install with: brew install jq"
    exit 1
fi

# Update the frameworks array in bundle.macOS
jq --argjson frameworks "$FRAMEWORKS_JSON" '.bundle.macOS.frameworks = $frameworks' "$TAURI_CONFIG" > "$TEMP_CONFIG"
mv "$TEMP_CONFIG" "$TAURI_CONFIG"

echo "✅ Updated frameworks in $TAURI_CONFIG with $(echo "$LOCAL_DYLIBS" | wc -l | tr -d ' ') local dylibs"

# Show summary
echo ""
echo "🎉 OpenCV dylib processing complete!"
echo "📊 Summary:"
echo "   - Found $(echo "$ALL_DYLIBS" | wc -l | tr -d ' ') homebrew dependencies"
echo "   - Copied to: $FRAMEWORKS_DIR/"
echo "   - Updated: $TAURI_CONFIG"
echo "   - Rewritten all internal references to use relative paths"