//! `psf-guard worker-service`: the worker as a Windows service, for a
//! computer that runs work while nobody is logged in.
//!
//! The app installs per user, in a folder its user can change, so the
//! service never runs the app's own files. `install` copies the binary to
//! `%ProgramFiles%\PSF Guard Worker` and the settings to
//! `%ProgramData%\PSF Guard Worker`, which only administrators and the
//! service's account can reach, and runs the service as its own virtual
//! account, `NT SERVICE\PSFGuardWorker`, rather than Local System. Running
//! `install` again updates the copies, such as after the app is updated.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[cfg(windows)]
mod permissions;
#[cfg(windows)]
mod process;

/// The service's name, as `sc.exe` and the Services console identify it.
#[cfg_attr(not(windows), allow(dead_code))]
const SERVICE_NAME: &str = "PSFGuardWorker";
#[cfg_attr(not(windows), allow(dead_code))]
const DISPLAY_NAME: &str = "PSF Guard Worker";
#[cfg_attr(not(windows), allow(dead_code))]
const DESCRIPTION: &str = "Runs stack builds and RC-Astro tools for a PSF Guard server.";
/// The folder under Program Files for the binary, and under ProgramData for
/// the settings, the task files and the logs.
#[cfg_attr(not(windows), allow(dead_code))]
const FOLDER: &str = "PSF Guard Worker";

/// Install the service with the settings at `settings`, or update it, and
/// start it.
pub fn install(settings: &Path) -> Result<()> {
    let settings =
        std::path::absolute(settings).with_context(|| format!("finding {}", settings.display()))?;
    let loaded = super::WorkerSettings::load(&settings)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No worker settings file was found at {}. Save worker settings in PSF Guard's \
             Settings › Workers › Worker on this computer, or choose a file with --config FILE",
            settings.display()
        )
    })?;
    loaded.validate()?;
    platform::install(loaded)
}

/// Stop the service and remove it, with its copies of the binary and the
/// settings.
pub fn uninstall() -> Result<()> {
    platform::uninstall()
}

/// Run as the service. Windows starts this; it is not for a terminal.
pub fn run(settings: PathBuf) -> Result<()> {
    platform::run(settings)
}

/// Whether the service is installed, and whether it runs: for the desktop
/// app, which shows a service worker in place of its own.
pub fn state() -> Option<ServiceState> {
    platform::state()
}

/// What the desktop app shows about the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    /// Starting, stopping, paused or resuming.
    Changing,
}

/// The settings the service runs with: the saved ones, with the service's
/// own task folder.
#[cfg_attr(not(windows), allow(dead_code))]
fn service_settings(mut settings: super::WorkerSettings, tasks: &Path) -> super::WorkerSettings {
    settings.work_dir = Some(tasks.display().to_string());
    settings
}

/// The arguments the service starts the binary with.
#[cfg_attr(not(windows), allow(dead_code))]
fn launch_arguments(settings: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "worker-service".into(),
        "run".into(),
        "--config".into(),
        settings.as_os_str().to_owned(),
    ]
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use windows_service::service::{
        Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl,
        ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
        ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState as State,
        ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::*;
    use crate::commands::worker::{run_with, WorkerControl, WorkerLaunch};

    /// How long the worker may take to stop: its tool stops within 30
    /// seconds, and it then leaves the server.
    const STOP_WAIT: Duration = Duration::from_secs(60);
    /// How long after a failure Windows starts the service again.
    const RESTART_AFTER: Duration = Duration::from_secs(30);
    /// How often a stop reports that it is still going.
    const STOP_PROGRESS: Duration = Duration::from_secs(5);

    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
    const REFUSED: &str = "Administrator access is required to manage the PSF Guard Worker service. Open PowerShell as administrator and run the command again.";

    static SETTINGS: OnceLock<PathBuf> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    /// Where the service's binary goes: only administrators can change it.
    fn binary() -> Result<PathBuf> {
        let folder = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .context("Windows did not report the Program Files folder")?;
        Ok(folder.join(FOLDER).join("psf-guard-worker.exe"))
    }

    /// The service's data folder: its settings, task files and logs.
    fn data() -> PathBuf {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join(FOLDER)
    }

    fn os_error(error: &windows_service::Error) -> Option<i32> {
        match error {
            windows_service::Error::Winapi(error) => error.raw_os_error(),
            _ => None,
        }
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access).map_err(|error| {
            if os_error(&error) == Some(ERROR_ACCESS_DENIED) {
                anyhow::anyhow!(REFUSED)
            } else {
                anyhow::Error::new(error).context("opening the Windows services")
            }
        })
    }

    /// The installed service, or `None` when there is none.
    fn open(manager: &ServiceManager, access: ServiceAccess) -> Result<Option<Service>> {
        match manager.open_service(SERVICE_NAME, access) {
            Ok(service) => Ok(Some(service)),
            Err(error) if os_error(&error) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(None),
            Err(error) if os_error(&error) == Some(ERROR_ACCESS_DENIED) => {
                Err(anyhow::anyhow!(REFUSED))
            }
            Err(error) => {
                Err(anyhow::Error::new(error).context("opening the PSF Guard Worker service"))
            }
        }
    }

    /// Stop the service and wait until its process has released the binary.
    fn stop_and_wait(service: &Service) -> Result<()> {
        let status = service.query_status()?;
        if status.current_state == State::Stopped {
            return Ok(());
        }
        // Keep a process handle before requesting Stop: the SCM no longer
        // reports a PID once the worker publishes its Stopped status, even
        // though the process can still be releasing its executable and logs.
        let process = status
            .process_id
            .map(process::Process::open)
            .transpose()?
            .flatten();
        if status.current_state != State::StopPending {
            service.stop()?;
        }
        let deadline = std::time::Instant::now() + STOP_WAIT + Duration::from_secs(10);
        while service.query_status()?.current_state != State::Stopped {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "The PSF Guard Worker service is still stopping; try again in a minute"
            );
            std::thread::sleep(Duration::from_millis(250));
        }
        if let Some(process) = process {
            process.wait(deadline.saturating_duration_since(std::time::Instant::now()))?;
        }
        Ok(())
    }

    pub fn install(settings: crate::commands::worker::WorkerSettings) -> Result<()> {
        let manager =
            manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let access = ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::QUERY_STATUS;
        let existing = open(&manager, access)?;
        // The copies can change only while the service is stopped.
        if let Some(service) = &existing {
            stop_and_wait(service)?;
        }

        let binary = binary()?;
        let data = data();
        let tasks = data.join("tasks");
        let saved = data.join("worker.json");
        std::fs::create_dir_all(binary.parent().expect("a Program Files folder"))?;
        std::fs::create_dir_all(&tasks)?;
        std::fs::create_dir_all(data.join("logs"))?;
        std::fs::copy(std::env::current_exe()?, &binary)
            .with_context(|| format!("copying PSF Guard to {}", binary.display()))?;
        service_settings(settings, &tasks).save(&saved)?;

        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: binary,
            launch_arguments: launch_arguments(&saved),
            dependencies: Vec::new(),
            // A virtual account of the service's own, which Windows creates
            // with it and which needs no password.
            account_name: Some(OsString::from(format!(r"NT SERVICE\{SERVICE_NAME}"))),
            account_password: None,
        };
        let service = match existing {
            Some(service) => {
                service.change_config(&info)?;
                service
            }
            None => manager
                .create_service(&info, access)
                .context("The service could not be created")?,
        };
        // Only now does the service's account exist to be granted access.
        permissions::lock_down(&data, &format!(r"NT SERVICE\{SERVICE_NAME}"))?;
        service.set_description(DESCRIPTION)?;
        service.set_delayed_auto_start(true)?;
        // A restart or a shutdown gives the worker time to leave the server.
        service.set_preshutdown_timeout(STOP_WAIT)?;
        // After a failure, such as a refused token, start again in 30
        // seconds, as the Linux service does.
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
            reboot_msg: None,
            command: None,
            actions: Some(
                (0..3)
                    .map(|_| ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: RESTART_AFTER,
                    })
                    .collect(),
            ),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)?;
        service.start::<&str>(&[])?;
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = open(
            &manager,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )?
        .context("The PSF Guard Worker service is not installed")?;
        stop_and_wait(&service)?;
        service.delete()?;
        drop(service);
        // The settings contain the token.
        let _ = std::fs::remove_dir_all(data());
        if let Ok(binary) = binary() {
            let _ = std::fs::remove_file(&binary);
            if let Some(folder) = binary.parent() {
                let _ = std::fs::remove_dir(folder);
            }
        }
        Ok(())
    }

    pub fn state() -> Option<ServiceState> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            .ok()?;
        Some(match service.query_status().ok()?.current_state {
            State::Running => ServiceState::Running,
            State::Stopped => ServiceState::Stopped,
            _ => ServiceState::Changing,
        })
    }

    pub fn run(settings: PathBuf) -> Result<()> {
        init_logging();
        let _ = SETTINGS.set(settings);
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).context(
            "This command is for Windows to start the service; use `psf-guard worker` in a terminal",
        )?;
        Ok(())
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(error) = serve() {
            tracing::error!("The worker service could not run: {error:#}");
        }
    }

    fn status(state: State, wait: Duration, checkpoint: u32, failed: bool) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == State::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::PRESHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            // A nonzero code tells Windows the service failed, so its
            // failure actions start it again.
            exit_code: if failed {
                ServiceExitCode::ServiceSpecific(1)
            } else {
                ServiceExitCode::Win32(0)
            },
            checkpoint,
            wait_hint: wait,
            process_id: None,
        }
    }

    fn serve() -> Result<()> {
        let control = WorkerControl::default();
        let handle: Arc<OnceLock<ServiceStatusHandle>> = Default::default();
        let stopping = Arc::new(AtomicBool::new(false));
        let registered = {
            let (control, handle, stopping) = (control.clone(), handle.clone(), stopping.clone());
            service_control_handler::register(SERVICE_NAME, move |event| match event {
                ServiceControl::Stop | ServiceControl::Preshutdown => {
                    if !stopping.swap(true, Ordering::SeqCst)
                        && let Some(handle) = handle.get().copied()
                    {
                        // Running tasks stop and the worker leaves the
                        // server, which queues them again. Until it has,
                        // Windows hears every few seconds that the stop is
                        // still going.
                        let stopping = stopping.clone();
                        std::thread::spawn(move || {
                            let checkpoint = AtomicU32::new(1);
                            while stopping.load(Ordering::SeqCst) {
                                let _ = handle.set_service_status(status(
                                    State::StopPending,
                                    STOP_PROGRESS * 2,
                                    checkpoint.fetch_add(1, Ordering::SeqCst),
                                    false,
                                ));
                                std::thread::sleep(STOP_PROGRESS);
                            }
                        });
                    }
                    control.quit();
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })?
        };
        let _ = handle.set(registered);
        let settings = SETTINGS.get().cloned();
        registered.set_service_status(status(State::Running, Duration::ZERO, 0, false))?;
        let ran = match settings {
            Some(settings) => tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    let ran = runtime.block_on(run_with(WorkerLaunch::Settings(settings), control));
                    // The worker has left the server; nothing it started is
                    // left to wait for.
                    runtime.shutdown_timeout(Duration::from_secs(5));
                    ran
                }),
            None => Err(anyhow::anyhow!(
                "The service started without its settings file"
            )),
        };
        // Log the reason before Windows hears the service stopped, since the
        // process may end as soon as it does.
        if let Err(error) = &ran {
            tracing::error!("The worker stopped: {error:#}");
        }
        stopping.store(false, Ordering::SeqCst);
        registered.set_service_status(status(State::Stopped, Duration::ZERO, 0, ran.is_err()))?;
        Ok(())
    }

    /// Log to files starting `worker` in `%ProgramData%\PSF Guard
    /// Worker\logs`: a service has no console.
    fn init_logging() {
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
        let file = crate::desktop_log::worker_file_appender(&data().join("logs"))
            .ok()
            .map(|appender| {
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_target(false)
                    .with_writer(appender)
                    .with_filter(
                        tracing_subscriber::filter::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
                    )
            });
        let _ = tracing_subscriber::registry().with(file).try_init();
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    const ELSEWHERE: &str = "This command is available only on Windows. On Linux, use the psf-guard-worker \
         systemd service. On macOS, open PSF Guard and choose Settings › Workers › Worker on this computer.";

    pub fn install(_settings: crate::commands::worker::WorkerSettings) -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }

    pub fn uninstall() -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }

    pub fn run(_settings: PathBuf) -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }

    pub fn state() -> Option<ServiceState> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_its_own_copy_of_the_settings_from_a_locked_folder() {
        let settings = Path::new(r"C:\ProgramData\PSF Guard Worker\worker.json");
        assert_eq!(
            launch_arguments(settings),
            [
                "worker-service",
                "run",
                "--config",
                r"C:\ProgramData\PSF Guard Worker\worker.json"
            ]
            .map(std::ffi::OsString::from)
        );
        let tasks = Path::new(r"C:\ProgramData\PSF Guard Worker\tasks");
        let saved = super::super::WorkerSettings {
            server: "http://observatory:3000".into(),
            token: "psfw_secret".into(),
            work_dir: Some(r"C:\Users\ann\scratch".into()),
            ..Default::default()
        };
        assert_eq!(
            service_settings(saved, tasks).work_dir.as_deref(),
            Some(r"C:\ProgramData\PSF Guard Worker\tasks")
        );
    }

    #[test]
    fn installing_needs_settings_that_can_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("worker.json");
        assert!(install(&missing)
            .unwrap_err()
            .to_string()
            .contains("Settings › Workers › Worker on this computer"));
        std::fs::write(
            &missing,
            r#"{"server": "observatory:3000", "token": "psfw_secret"}"#,
        )
        .unwrap();
        assert!(install(&missing)
            .unwrap_err()
            .to_string()
            .contains("http://"));
    }
}
