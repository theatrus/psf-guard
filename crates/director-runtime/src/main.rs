#[cfg(windows)]
mod windows_host {
    use std::{os::windows::io::AsRawHandle, time::Duration};
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

    pub async fn run() -> Result<(), &'static str> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if !matches!(args.len(), 4 | 6)
            || args[0] != "--pipe"
            || args[2] != "--parent-pid"
            || (args.len() == 6 && args[4] != "--state-directory")
        {
            return Err(
                "usage: psf-guard-director-runtime --pipe <local-pipe-name> --parent-pid <pid> [--state-directory <absolute-directory>]",
            );
        }
        let Some(suffix) = args[1].strip_prefix("psf-guard-director-") else {
            return Err("invalid local pipe name");
        };
        if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid local pipe name");
        }
        let expected_parent: u32 = args[3].parse().map_err(|_| "invalid parent process ID")?;
        if expected_parent == 0 {
            return Err("invalid parent process ID");
        }
        let path = format!(r"\\.\pipe\{}", args[1]);
        let pipe = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                // Tokio defaults to SECURITY_IDENTIFICATION, preventing server
                // impersonation with the client's token.
                match ClientOptions::new().open(&path) {
                    Ok(pipe) => return Ok(pipe),
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.raw_os_error() == Some(231) =>
                    {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(_) => return Err("failed to open local control pipe"),
                }
            }
        })
        .await
        .map_err(|_| "control pipe connection timed out")??;
        let mut server_pid = 0;
        // The handle is owned by `pipe` and remains valid for this call.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_pid) } == 0
            || server_pid != expected_parent
        {
            return Err("control pipe parent identity mismatch");
        }
        // Do not touch storage until the process peer is verified.
        let storage = if args.len() == 6 {
            Some(
                psf_guard_director_runtime::storage::Storage::acquire_for_startup(
                    std::path::Path::new(&args[5]),
                )
                .await
                .map_err(|error| match error {
                    psf_guard_director_runtime::storage::StorageError::Busy => {
                        "execution storage busy after startup wait"
                    }
                    psf_guard_director_runtime::storage::StorageError::InvalidDirectory => {
                        "execution storage directory invalid"
                    }
                    _ => "execution storage unavailable",
                })?,
            )
        } else {
            None
        };
        psf_guard_director_runtime::serve_with_storage(pipe, storage)
            .await
            .map_err(|_| "control protocol failed")
    }
}

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match windows_host::run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            // Never log input frames, credentials, or source paths.
            eprintln!("director-runtime: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("director-runtime: the NINA named-pipe host requires Windows");
    std::process::ExitCode::from(2)
}
