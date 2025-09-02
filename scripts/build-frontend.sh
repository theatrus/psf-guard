#!/bin/bash

# Build script for PSF Guard frontend

set -e

echo "Building PSF Guard frontend..."

# Change to the static directory
cd "$(dirname "$0")/../static"

# Install dependencies if needed
if [ ! -d "node_modules" ]; then
    echo "Installing dependencies..."
    npm install
fi

# Run the build
echo "Building React app..."
npm run build

# Create a dist directory in the project root if it doesn't exist
mkdir -p ../dist/static

# Copy the built files
echo "Copying built files..."
cp -r dist/* ../dist/static/

echo "Frontend build complete!"
echo "Built files are in: dist/static/"
echo ""
echo "To serve the production build, run:"
echo "  cargo run --release -- server <database> <image_dir> --static-dir dist/static"