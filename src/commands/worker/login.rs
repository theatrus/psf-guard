//! Start the desktop app's worker when the user logs in: a LaunchAgent on
//! macOS, and a value under the user's `Run` key on Windows. Both start
//! `psf-guard worker --tray` with the worker's settings file. Elsewhere the
//! worker runs as a service instead; see `docs/REMOTE_WORKERS.md`.

use std::path::Path;

use anyhow::Result;

/// The LaunchAgent's label, and the name of the `Run` value.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LABEL: &str = "com.theatrus.psf-guard.worker";
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_VALUE: &str = "PSF Guard Worker";

/// Whether this system can start the worker at login.
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// Whether the worker starts at login.
pub fn enabled() -> bool {
    platform::enabled()
}

/// Start the worker at login: `exe worker --tray --config settings`.
pub fn enable(exe: &Path, settings: &Path) -> Result<()> {
    platform::enable(exe, settings)
}

/// Stop starting the worker at login. A running worker keeps running.
pub fn disable() -> Result<()> {
    platform::disable()
}

/// The LaunchAgent property list that starts the worker.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent(exe: &Path, settings: &Path) -> String {
    let escape = |path: &Path| {
        path.display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
		<string>worker</string>
		<string>--tray</string>
		<string>--config</string>
		<string>{}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        escape(exe),
        escape(settings)
    )
}

/// The command line stored in the `Run` value.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_command(exe: &Path, settings: &Path) -> String {
    format!(
        "\"{}\" worker --tray --config \"{}\"",
        exe.display(),
        settings.display()
    )
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use anyhow::Context;

    fn plist() -> Result<std::path::PathBuf> {
        Ok(dirs::home_dir()
            .context("This user has no home folder")?
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    pub fn enabled() -> bool {
        plist().is_ok_and(|path| path.exists())
    }

    pub fn enable(exe: &Path, settings: &Path) -> Result<()> {
        let path = plist()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, launch_agent(exe, settings))
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn disable() -> Result<()> {
        match std::fs::remove_file(plist()?) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn enabled() -> bool {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        let mut size = 0u32;
        // SAFETY: the key and value names are NUL-terminated, and a null
        // buffer asks only for the size.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        status == ERROR_SUCCESS
    }

    pub fn enable(exe: &Path, settings: &Path) -> Result<()> {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        let command = wide(&run_command(exe, settings));
        // SAFETY: the names are NUL-terminated, and the data is the
        // NUL-terminated command, sized in bytes.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * std::mem::size_of::<u16>()) as u32,
            )
        };
        anyhow::ensure!(
            status == ERROR_SUCCESS,
            "Windows did not save the startup entry (error {status})"
        );
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        // SAFETY: the key and value names are NUL-terminated.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        anyhow::ensure!(
            status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND,
            "Windows did not remove the startup entry (error {status})"
        );
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::*;

    pub fn enabled() -> bool {
        false
    }

    pub fn enable(_exe: &Path, _settings: &Path) -> Result<()> {
        anyhow::bail!(
            "Starting the worker at login is available on macOS and Windows; \
             on Linux, run it with the psf-guard-worker systemd unit"
        )
    }

    pub fn disable() -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_entries_start_the_tray_with_the_settings_file() {
        let exe = Path::new("/Applications/PSF Guard.app/Contents/MacOS/psf-guard");
        let settings = Path::new("/Users/a&b/Library/Application Support/psf-guard/worker.json");
        let plist = launch_agent(exe, settings);
        assert!(plist.contains(
            "<string>/Applications/PSF Guard.app/Contents/MacOS/psf-guard</string>\n\t\t<string>worker</string>\n\t\t<string>--tray</string>\n\t\t<string>--config</string>"
        ));
        assert!(plist.contains("<string>/Users/a&amp;b/Library/"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
        let exe = Path::new(r"C:\Program Files\PSF Guard\psf-guard.exe");
        let settings = Path::new(r"C:\Users\ann\AppData\Roaming\psf-guard\worker.json");
        assert_eq!(
            run_command(exe, settings),
            r#""C:\Program Files\PSF Guard\psf-guard.exe" worker --tray --config "C:\Users\ann\AppData\Roaming\psf-guard\worker.json""#
        );
    }
}
