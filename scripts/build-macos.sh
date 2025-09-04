#!/bin/bash
# Build helper script for macOS with OpenCV support

# Set up libclang path for OpenCV
export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/"

# Check if the path exists, otherwise try Command Line Tools path
if [ ! -d "$DYLD_FALLBACK_LIBRARY_PATH" ]; then
    export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/usr/lib/"
fi

echo "Using DYLD_FALLBACK_LIBRARY_PATH: $DYLD_FALLBACK_LIBRARY_PATH"

export OPENCV_LINK_LIBS="static=opencv_core,static=opencv_imgproc,static=opencv_imgcodecs,static=opencv_ximgproc"

# Run the build with all arguments passed through
cargo "$@"
