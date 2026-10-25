//! A worker's settings file: what `psf-guard worker --config` reads, and
//! what the desktop app writes for the worker it runs on this computer.
//! The file contains the worker's token, so only its user can read it.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::WorkerFilesArg;
use crate::remote_work::protocol::{FileMode, PathMap, MAX_SLOTS};

/// A worker's settings, as its settings file stores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSettings {
    /// The server's address, such as `http://192.168.1.10:3000`.
    pub server: String,
    /// The worker token from the server's Settings › Workers.
    pub token: String,
    /// The name the server's Settings › Workers shows. Empty uses this
    /// computer's host name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub files: FileMode,
    /// Where each server folder is on this computer, for shared files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps: Vec<PathMap>,
    /// Where tasks keep their files while they run. Empty uses the system
    /// temporary folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// The rc-astro executable. Empty finds the standard install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc_astro: Option<String>,
    /// The work the worker accepts, as `--work` takes it. Empty accepts
    /// everything the worker can run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work: Vec<String>,
    #[serde(default = "one_slot")]
    pub slots: usize,
}

fn one_slot() -> usize {
    1
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            server: String::new(),
            token: String::new(),
            name: None,
            files: FileMode::Remote,
            maps: Vec::new(),
            work_dir: None,
            rc_astro: None,
            work: Vec::new(),
            slots: 1,
        }
    }
}

impl WorkerSettings {
    /// This user's worker settings file, in PSF Guard's settings folder on
    /// this computer: on Windows, the local profile rather than the roaming
    /// one, since a worker belongs to one computer.
    pub fn default_path() -> Result<PathBuf> {
        Ok(dirs::config_local_dir()
            .context("This system has no settings folder")?
            .join("psf-guard")
            .join("worker.json"))
    }

    /// Read the settings at `path`, or `None` when there is no file.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let body = match std::fs::read(path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        let settings: Self = serde_json::from_slice(&body)
            .with_context(|| format!("reading the worker settings at {}", path.display()))?;
        Ok(Some(settings))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let mut body = serde_json::to_vec_pretty(self)?;
        body.push(b'\n');
        write_private(path, &body)
    }

    /// Check what the worker needs before it starts, with a message that
    /// explains how to fix each problem.
    pub fn validate(&self) -> Result<()> {
        let server = self.server.trim();
        anyhow::ensure!(
            server.starts_with("http://") || server.starts_with("https://"),
            "Enter the server's address, starting with http:// or https://"
        );
        anyhow::ensure!(
            self.token
                .trim()
                .starts_with(crate::remote_work::protocol::TOKEN_PREFIX),
            "Enter a worker token from the server's Settings › Workers; it starts with {}",
            crate::remote_work::protocol::TOKEN_PREFIX
        );
        anyhow::ensure!(
            (1..=MAX_SLOTS).contains(&self.slots),
            "Choose from 1 to {MAX_SLOTS} tasks at once"
        );
        crate::remote_work::WorkChoice::parse(&self.work).map_err(anyhow::Error::msg)?;
        for map in &self.maps {
            anyhow::ensure!(
                !map.server.trim().is_empty() && !map.local.trim().is_empty(),
                "Each shared folder needs both the server's path and this computer's path"
            );
        }
        Ok(())
    }

    /// The worker's arguments, as the command line would give them.
    pub fn into_args(self) -> super::WorkerArgs {
        let blank_is_none = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
        super::WorkerArgs {
            server: self.server,
            token: Some(self.token),
            name: blank_is_none(self.name),
            files: match self.files {
                FileMode::Remote => WorkerFilesArg::Remote,
                FileMode::Shared => WorkerFilesArg::Shared,
            },
            maps: self
                .maps
                .into_iter()
                .map(|map| format!("{}={}", map.server.trim(), map.local.trim()))
                .collect(),
            work_dir: blank_is_none(self.work_dir),
            rc_astro: blank_is_none(self.rc_astro),
            work: self.work,
            slots: self.slots,
        }
    }
}

/// Write `body` to `path` through a temporary file beside it, readable and
/// writable only by this user on Unix. On Windows the file takes the
/// folder's permissions, which for a user's settings folder are the user's.
pub(super) fn write_private(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path.parent().context("the settings file has no folder")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(body)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> WorkerSettings {
        WorkerSettings {
            server: "http://observatory:3000".into(),
            token: "psfw_secret".into(),
            name: Some("  ".into()),
            files: FileMode::Shared,
            maps: vec![PathMap {
                server: "/srv/astro".into(),
                local: "Z:\\astro".into(),
            }],
            work: vec!["stack".into()],
            slots: 2,
            ..Default::default()
        }
    }

    #[test]
    fn settings_round_trip_only_their_user_can_read_them_and_become_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("psf-guard").join("worker.json");
        assert_eq!(WorkerSettings::load(&path).unwrap(), None);
        settings().save(&path).unwrap();
        assert_eq!(WorkerSettings::load(&path).unwrap(), Some(settings()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let args = settings().into_args();
        assert_eq!(args.name, None, "a blank name uses the host name");
        assert_eq!(args.files, WorkerFilesArg::Shared);
        assert_eq!(args.maps, ["/srv/astro=Z:\\astro"]);
        assert_eq!(args.slots, 2);
    }

    #[test]
    fn settings_that_cannot_run_explain_what_to_fix() {
        let refused = |change: fn(&mut WorkerSettings)| {
            let mut settings = settings();
            change(&mut settings);
            settings.validate().unwrap_err().to_string()
        };
        assert!(refused(|s| s.server = "observatory:3000".into()).contains("http://"));
        assert!(refused(|s| s.token = "secret".into()).contains("psfw_"));
        assert!(refused(|s| s.slots = 0).contains("1 to 16"));
        assert!(refused(|s| s.work = vec!["painting".into()]).contains("painting"));
        assert!(refused(|s| s.maps[0].local = String::new()).contains("both"));
    }
}
