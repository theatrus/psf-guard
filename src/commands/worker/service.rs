//! `psf-guard worker-service`: the worker as a Windows service, for a
//! computer that runs work while nobody is logged in. The service runs
//! `psf-guard worker-service run --config FILE` as Local System, with the
//! settings the desktop app saved, and starts with Windows. It has no tray;
//! a desktop that someone logs in to can run the tray worker instead.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// The service's name, as `sc.exe` and the Services console identify it.
#[cfg_attr(not(windows), allow(dead_code))]
const SERVICE_NAME: &str = "PSFGuardWorker";
#[cfg_attr(not(windows), allow(dead_code))]
const DISPLAY_NAME: &str = "PSF Guard Worker";
#[cfg_attr(not(windows), allow(dead_code))]
const DESCRIPTION: &str = "Runs stack builds and RC-Astro tools for a PSF Guard server.";

/// Install the service with the settings at `settings`, and start it.
pub fn install(settings: &Path) -> Result<()> {
    let loaded = super::WorkerSettings::load(settings)?.ok_or_else(|| {
        anyhow::anyhow!(
            "There are no worker settings at {}; save them in PSF Guard's Settings › Run a \
             worker, or give a settings file with --config",
            settings.display()
        )
    })?;
    loaded.validate()?;
    platform::install(settings)
}

/// Stop the service and remove it.
pub fn uninstall() -> Result<()> {
    platform::uninstall()
}

/// Run as the service. Windows starts this; it is not for a terminal.
pub fn run(settings: PathBuf) -> Result<()> {
    platform::run(settings)
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
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::Context;
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
        ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::*;
    use crate::commands::worker::{run_with, WorkerControl, WorkerLaunch};

    /// How long Windows waits for the worker to leave the server on a stop.
    const STOP_WAIT: Duration = Duration::from_secs(40);
    /// How long after a failure Windows starts the service again.
    const RESTART_AFTER: Duration = Duration::from_secs(30);

    static SETTINGS: OnceLock<PathBuf> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn install(settings: &Path) -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .context("Windows refused to install the service; run this from a terminal opened as administrator")?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: std::env::current_exe()?,
            launch_arguments: launch_arguments(settings),
            dependencies: Vec::new(),
            // Local System.
            account_name: None,
            account_password: None,
        };
        let service = manager
            .create_service(
                &info,
                ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
            )
            .context("The service could not be created; if it exists, uninstall it first")?;
        service.set_description(DESCRIPTION)?;
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
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("Windows refused to open the services; run this from a terminal opened as administrator")?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
            )
            .context("The PSF Guard Worker service is not installed")?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
            let deadline = std::time::Instant::now() + STOP_WAIT;
            while service.query_status()?.current_state != ServiceState::Stopped
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        service.delete()?;
        Ok(())
    }

    pub fn run(settings: PathBuf) -> Result<()> {
        init_logging();
        let _ = SETTINGS.set(settings);
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("This command is for Windows to start the service; use `psf-guard worker` in a terminal")?;
        Ok(())
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(error) = serve() {
            tracing::error!("The worker service stopped: {error:#}");
        }
    }

    fn status(state: ServiceState, wait: Duration, exit: u32) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(exit),
            checkpoint: 0,
            wait_hint: wait,
            process_id: None,
        }
    }

    fn serve() -> Result<()> {
        let control = WorkerControl::default();
        let handle: std::sync::Arc<OnceLock<ServiceStatusHandle>> = Default::default();
        let registered = {
            let (control, handle) = (control.clone(), handle.clone());
            service_control_handler::register(SERVICE_NAME, move |event| match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    if let Some(handle) = handle.get() {
                        let _ = handle.set_service_status(status(
                            ServiceState::StopPending,
                            STOP_WAIT,
                            0,
                        ));
                    }
                    // Running tasks stop and the worker leaves the server,
                    // which queues them again.
                    control.quit();
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })?
        };
        let _ = handle.set(registered);
        registered.set_service_status(status(ServiceState::Running, Duration::ZERO, 0))?;
        let settings = SETTINGS
            .get()
            .cloned()
            .context("The service started without its settings file")?;
        let ran = tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|runtime| {
                runtime.block_on(run_with(WorkerLaunch::Settings(settings), control))
            });
        // A nonzero exit tells Windows the service failed, so it starts it
        // again.
        registered.set_service_status(status(
            ServiceState::Stopped,
            Duration::ZERO,
            u32::from(ran.is_err()),
        ))?;
        ran
    }

    /// Log to files starting `psf-guard-worker` in
    /// `%ProgramData%\PSF Guard\logs`: a service has no console, and Local
    /// System's own folders are hard to find.
    fn init_logging() {
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
        let dir = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("PSF Guard")
            .join("logs");
        let file = crate::desktop_log::worker_file_appender(&dir)
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

    const ELSEWHERE: &str = "The worker runs as a service this way only on Windows. On Linux, use \
         the psf-guard-worker systemd service; on a Mac, open PSF Guard's Settings › Run a worker";

    pub fn install(_settings: &Path) -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }

    pub fn uninstall() -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }

    pub fn run(_settings: PathBuf) -> Result<()> {
        anyhow::bail!(ELSEWHERE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_starts_the_worker_with_its_settings_file() {
        let settings = Path::new(r"C:\Users\ann\AppData\Roaming\psf-guard\worker.json");
        assert_eq!(
            launch_arguments(settings),
            [
                "worker-service",
                "run",
                "--config",
                r"C:\Users\ann\AppData\Roaming\psf-guard\worker.json"
            ]
            .map(std::ffi::OsString::from)
        );
    }

    #[test]
    fn installing_needs_settings_that_can_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("worker.json");
        assert!(install(&missing)
            .unwrap_err()
            .to_string()
            .contains("Settings › Run a worker"));
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
