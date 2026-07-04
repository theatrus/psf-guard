//! Dedicated console-subsystem CLI entry point.
//!
//! The default `psf-guard` binary is built with the `tauri` feature for the
//! desktop app, which stamps it with `windows_subsystem = "windows"` (a GUI
//! subsystem app). A GUI-subsystem process does not attach to the parent
//! console on Windows, so its stdout/stderr are invisible when run from a
//! terminal — a poor CLI experience even though `psf-guard.exe` is dual-mode.
//!
//! This target is intentionally minimal and never sets `windows_subsystem`, so
//! it stays a console application. It is what the Windows installer ships as
//! `psf-guard-cli.exe` alongside the GUI app, and what the standalone
//! `psf-guard-*-x64` release binaries are built from. It links no Tauri code.
fn main() -> anyhow::Result<()> {
    psf_guard::cli_main::main()
}
