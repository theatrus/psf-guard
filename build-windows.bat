
set OPENCV_LINK_DIRS=C:\vcpkg\installed\x64-windows\lib
set OPENCV_DISABLE_PROBES=pkg_config,cmake,vcpkg_cmake
set OpenCV_DIR=C:\vcpkg\installed\x64-windows
set LIBCLANG_PATH=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin
set PATH=C:\vcpkg;%PATH%
set VCPKG_ROOT=C:\vcpkg
rem set VCPKGRS_DYNAMIC=1

cargo build --release
cargo tauri build
