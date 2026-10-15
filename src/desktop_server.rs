//! Desktop-only server preferences, separate from the shared catalog registry
//! and the CLI server's TOML configuration.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Settings {
    /// None lets the operating system choose a port on each start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_port: Option<u16>,
}

impl Settings {
    pub fn path(registry_path: &Path) -> PathBuf {
        registry_path.with_file_name("desktop.json")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let body = match std::fs::read(path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let settings: Self = serde_json::from_slice(&body)
            .with_context(|| format!("reading desktop settings at {}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(port) = self.server_port
            && port < 1024
        {
            bail!("Choose a server port from 1024 to 65535, or Automatic.");
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().context("desktop settings directory")?;
        std::fs::create_dir_all(parent).context("creating desktop settings directory")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, self)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .with_context(|| format!("saving {}", path.display()))?;
        Ok(())
    }

    /// Keep this listener through server startup: probing and releasing a port
    /// would let another process take it before the server can bind it.
    pub fn bind(&self) -> Result<TcpListener> {
        self.validate()?;
        let port = self.server_port.unwrap_or(0);
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .with_context(|| match self.server_port {
                Some(port) => format!(
                    "Could not start the desktop server on 127.0.0.1:{port}. \
                     The port may already be in use. Choose another port or Automatic in Server settings."
                ),
                None => "Could not choose a localhost port for the desktop server.".to_string(),
            })?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_installations_keep_automatic_ports() {
        let dir = tempfile::tempdir().unwrap();
        let path = Settings::path(&dir.path().join("config.json"));
        assert_eq!(path, dir.path().join("desktop.json"));
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.server_port, None);
        let listener = settings.bind().unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    #[test]
    fn saved_ports_survive_restarts_and_can_return_to_automatic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/desktop.json");
        let listener = Settings::default().bind().unwrap();
        let port = listener.local_addr().unwrap().port();
        let settings = Settings {
            server_port: Some(port),
        };
        settings.save(&path).unwrap();
        drop(listener);
        let reloaded = Settings::load(&path).unwrap();
        assert_eq!(reloaded, settings);
        let restarted = reloaded.bind().unwrap();
        assert_eq!(restarted.local_addr().unwrap().port(), port);
        drop(restarted);
        Settings::default().save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap().server_port, None);
    }

    #[test]
    fn an_occupied_fixed_port_fails_without_falling_back() {
        let listener = Settings::default().bind().unwrap();
        let port = listener.local_addr().unwrap().port();
        let error = Settings {
            server_port: Some(port),
        }
        .bind()
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(&format!("127.0.0.1:{port}")), "{message}");
        assert!(message.contains("in use"), "{message}");
        assert!(message.contains("Server settings"), "{message}");
    }

    #[test]
    fn invalid_fixed_ports_cannot_replace_saved_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.json");
        Settings {
            server_port: Some(3000),
        }
        .save(&path)
        .unwrap();
        for port in [0, 1, 1023] {
            assert!(Settings {
                server_port: Some(port)
            }
            .save(&path)
            .is_err());
        }
        assert_eq!(Settings::load(&path).unwrap().server_port, Some(3000));
    }

    #[test]
    fn malformed_settings_do_not_silently_choose_another_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.json");
        for body in ["broken", r#"{"server_port":0}"#, r#"{"server_port":65536}"#] {
            std::fs::write(&path, body).unwrap();
            assert!(Settings::load(&path).is_err());
        }
    }
}
