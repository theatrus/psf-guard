@echo off
rem Build the Windows installers after building static/dist.
cargo tauri build --config tauri.bundle-windows.json %*
