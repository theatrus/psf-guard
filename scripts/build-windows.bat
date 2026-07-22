rem Build helper for Windows (no special environment needed since the
rem OpenCV dependency was removed).

cargo build --release
cargo tauri build
