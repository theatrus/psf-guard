//! The desktop app's log file. A Tauri window has no console on Windows or
//! macOS, so what the server logs (a frame a stack turned away and why, a
//! master that would not build) would otherwise be lost. The file sits where
//! Tauri keeps an app's logs and rolls over daily, keeping a week.

use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// The bundle identifier in `tauri.conf.json`; Tauri names the folder after it.
pub const IDENTIFIER: &str = "com.theatrus.psf-guard";
/// Daily files kept; older ones are deleted as new days start.
pub const KEPT_LOG_FILES: usize = 7;
const FILE_PREFIX: &str = "psf-guard";
const FILE_SUFFIX: &str = "log";

/// Where the log files go, the folder Tauri's `app_log_dir` names:
/// `~/Library/Logs/<identifier>` on macOS, `<local data>/<identifier>/logs`
/// elsewhere (`%LOCALAPPDATA%` on Windows, `~/.local/share` on Linux).
pub fn log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join("Library").join("Logs").join(IDENTIFIER))
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::data_local_dir().map(|data| data.join(IDENTIFIER).join("logs"))
    }
}

/// A daily file appender in `dir`, created if missing, keeping
/// [`KEPT_LOG_FILES`] files.
pub fn file_appender(dir: &Path) -> std::io::Result<RollingFileAppender> {
    std::fs::create_dir_all(dir)?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(KEPT_LOG_FILES)
        .build(dir)
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn the_identifier_matches_the_bundle() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
    }

    #[test]
    fn the_appender_writes_a_dated_file_in_a_new_folder() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        let mut appender = file_appender(&dir).unwrap();
        appender.write_all(b"stack rejected a frame\n").unwrap();
        appender.flush().unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(files.len(), 1);
        assert!(
            files[0].starts_with("psf-guard.") && files[0].ends_with(".log"),
            "{files:?}"
        );
        let text = std::fs::read_to_string(dir.join(&files[0])).unwrap();
        assert!(text.contains("stack rejected a frame"));
    }

    #[test]
    fn the_folder_is_named_after_the_app() {
        let dir = log_dir().expect("a home folder");
        assert!(dir.components().any(|part| part.as_os_str() == IDENTIFIER));
    }
}
