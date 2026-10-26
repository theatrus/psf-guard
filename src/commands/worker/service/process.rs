//! Wait for a stopped service's process before replacing its executable.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

pub(super) struct Process(OwnedHandle);

impl Process {
    pub(super) fn open(pid: u32) -> Result<Option<Self>> {
        // Only synchronization access is needed; this handle cannot change
        // the service process or inspect its memory.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                // The worker exited between the status query and OpenProcess.
                return Ok(None);
            }
            return Err(error).context("opening the worker service process to wait for its exit");
        }
        // OpenProcess returned a new owned handle, closed by OwnedHandle.
        Ok(Some(Self(unsafe { OwnedHandle::from_raw_handle(handle) })))
    }

    pub(super) fn wait(&self, timeout: Duration) -> Result<()> {
        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => anyhow::bail!(
                "The PSF Guard Worker process is still stopping; try again in a minute"
            ),
            _ => Err(std::io::Error::last_os_error())
                .context("waiting for the worker service process to exit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn waits_for_process_exit_after_the_worker_has_reported_stopped() {
        // Hold the child at stdin, as a worker can remain alive during
        // shutdown after publishing its final status to the SCM.
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::In.ReadLine() | Out-Null",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let process = Process::open(child.id()).unwrap().unwrap();
        assert!(process.wait(Duration::ZERO).is_err());
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        process.wait(Duration::from_secs(15)).unwrap();
        assert!(child.wait().unwrap().success());
    }
}
