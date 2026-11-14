//! `psf-guard worker-service`: the worker as a Windows service, for a
//! computer that runs work while nobody is logged in.
//!
//! The app installs per user, in a folder its user can change, so the
//! service never runs the app's own files. `install` copies the binary to
//! `%ProgramFiles%\PSF Guard Worker` and the settings to
//! `%ProgramData%\PSF Guard Worker`, which only administrators and the
//! service's account can reach. The service runs as the account that
//! installs it, so it has that person's RC-Astro license and network
//! shares; `--virtual-account` runs it as its own account,
//! `NT SERVICE\PSFGuardWorker`, as earlier installs did. Running
//! `install` again updates the copies, such as after the app is updated,
//! and moves an older install off the virtual account.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[cfg(windows)]
mod account;
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

/// The account the service runs as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RunAs {
    /// The account running `install`.
    #[default]
    Installer,
    /// A named account, such as `PC\ann` or `DOMAIN\ann`.
    Account(String),
    /// The service's own virtual account, `NT SERVICE\PSFGuardWorker`,
    /// which needs no password but has no one's license or shares.
    Virtual,
}

/// How `install` gets a user account's password.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Password {
    /// Ask in the terminal, only when the account changes.
    #[default]
    WhenChanged,
    /// Ask in the terminal even when the account is unchanged, such as
    /// after its Windows password changed.
    Again,
    /// Read one line from standard input, for a script.
    Stdin,
}

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub run_as: RunAs,
    pub password: Password,
}

/// What `install` did about the service's account, for the command's
/// output.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// The account the service runs as.
    pub account: String,
    /// The account the service ran as before, when it changed.
    pub replaced: Option<String>,
    /// Whether this install gave the account "Log on as a service".
    pub granted_service_logon: bool,
    /// Whether the old virtual account's profile was deleted.
    pub deleted_virtual_profile: bool,
}

/// Install the service with the settings at `settings`, or update it, and
/// start it.
pub fn install(settings: &Path, options: &InstallOptions) -> Result<Installed> {
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
    platform::install(loaded, options)
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

/// What `install` records about the account, in the service's data folder,
/// so that a later install or uninstall can undo what it did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
struct AccountRecord {
    sid: String,
    /// "Log on as a service" was given by an install, not by the person.
    #[serde(default)]
    granted_service_logon: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl AccountRecord {
    const FILE: &'static str = "account.json";

    fn load(data: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(data.join(Self::FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, data: &Path) -> Result<()> {
        let path = data.join(Self::FILE);
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, &path).with_context(|| format!("writing {}", path.display()))
    }
}

/// Whether a service account name is the service's own virtual account.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_virtual_account(name: &str) -> bool {
    name.eq_ignore_ascii_case(&format!(r"NT SERVICE\{SERVICE_NAME}"))
}

/// Read a password from one line of `input`, without its line ending.
#[cfg_attr(not(windows), allow(dead_code))]
fn read_password_line(mut input: impl std::io::BufRead) -> Result<String> {
    let mut line = String::new();
    input.read_line(&mut line).context("reading the password")?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    anyhow::ensure!(
        !password.is_empty(),
        "No password was given on standard input"
    );
    Ok(password)
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
    const ERROR_SERVICE_LOGON_FAILED: i32 = 1069;
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

    /// The account to run as, and its password when the service's account
    /// changes to it.
    struct Target {
        account: account::Account,
        password: Option<String>,
    }

    fn virtual_account_name() -> String {
        format!(r"NT SERVICE\{SERVICE_NAME}")
    }

    /// The account the installed service runs as, or `None` when Windows
    /// cannot say or the account no longer exists.
    fn installed_account(service: &Service) -> Option<account::Account> {
        let name = service.query_config().ok()?.account_name?;
        account::lookup(&name.to_string_lossy()).ok()
    }

    fn ask_password(account: &account::Account, how: Password) -> Result<String> {
        if how == Password::Stdin {
            return read_password_line(std::io::stdin().lock());
        }
        let password = rpassword::prompt_password(format!(
            "Windows password for {} (the service logs on with it): ",
            account.name
        ))
        .context("reading the password; to give it from a script, use --password-stdin")?;
        anyhow::ensure!(!password.is_empty(), "No password was entered");
        Ok(password)
    }

    pub fn install(
        settings: crate::commands::worker::WorkerSettings,
        options: &InstallOptions,
    ) -> Result<Installed> {
        let manager =
            manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let access = ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::QUERY_CONFIG
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::QUERY_STATUS;
        let existing = open(&manager, access)?;
        let data = data();
        let previous = existing.as_ref().and_then(installed_account);
        let record = AccountRecord::load(&data);

        // Settle the account, its right and its password while the old
        // service still runs, so a refused password changes nothing.
        let wanted = match &options.run_as {
            RunAs::Virtual => None,
            RunAs::Installer => Some(account::current()?),
            RunAs::Account(name) => Some(account::lookup(name)?),
        };
        let mut granted = false;
        let target = match wanted {
            None => None,
            Some(wanted) => {
                anyhow::ensure!(
                    wanted.user,
                    "{} is not a user account. Choose a person's account with --account, \
                     or install with --virtual-account.",
                    wanted.name
                );
                let unchanged = previous.as_ref().is_some_and(|old| old.sid == wanted.sid);
                let password = if unchanged && options.password == Password::WhenChanged {
                    None
                } else {
                    Some(ask_password(&wanted, options.password)?)
                };
                granted = account::grant_service_logon(&wanted)?;
                if let Some(password) = &password
                    && let Err(error) = account::check_password(&wanted, password)
                {
                    if granted {
                        let _ = account::revoke_service_logon(&wanted.sid);
                    }
                    return Err(error);
                }
                Some(Target {
                    account: wanted,
                    password,
                })
            }
        };
        let virtual_name = virtual_account_name();
        let new_sid = match &target {
            Some(target) => target.account.sid.clone(),
            // Windows creates the virtual account with the service.
            None => String::new(),
        };
        let changed = match (&previous, &target) {
            (Some(old), Some(new)) => old.sid != new.account.sid,
            (Some(old), None) => !old.name.eq_ignore_ascii_case(&virtual_name),
            (None, _) => existing.is_some(),
        };

        // The copies can change only while the service is stopped.
        if let Some(service) = &existing {
            stop_and_wait(service)?;
        }

        let binary = binary()?;
        let tasks = data.join("tasks");
        let saved = data.join("worker.json");
        std::fs::create_dir_all(binary.parent().expect("a Program Files folder"))?;
        std::fs::create_dir_all(&tasks)?;
        std::fs::create_dir_all(data.join("logs"))?;
        std::fs::copy(std::env::current_exe()?, &binary)
            .with_context(|| format!("copying PSF Guard to {}", binary.display()))?;
        service_settings(settings, &tasks).save(&saved)?;

        // A new service, or a changed account, gets the account's name and
        // password. An unchanged account keeps the password Windows stored.
        let (account_name, account_password) = match &target {
            Some(target) => match &target.password {
                Some(password) => (
                    Some(OsString::from(&target.account.name)),
                    Some(OsString::from(password)),
                ),
                None => (None, None),
            },
            None => (Some(OsString::from(&virtual_name)), None),
        };
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: binary,
            launch_arguments: launch_arguments(&saved),
            dependencies: Vec::new(),
            account_name,
            account_password,
        };
        let service = match existing {
            Some(service) => {
                service.change_config(&info)?;
                service
            }
            // A new service has no account yet, so a password was asked.
            None => manager
                .create_service(&info, access)
                .context("The service could not be created")?,
        };
        drop(info);

        // Only now does a new virtual account exist to be granted access.
        let grantee = match &target {
            Some(_) => format!("*{new_sid}"),
            None => virtual_name.clone(),
        };
        // The old account loses the folder, which holds the token.
        let revoked = if changed {
            previous.as_ref().map(|old| format!("*{}", old.sid))
        } else {
            None
        };
        permissions::lock_down(&data, &grantee, revoked.as_deref())?;
        let mut deleted_virtual_profile = false;
        if changed && let Some(old) = &previous {
            // The old account's task files go; the server queues them again.
            let _ = std::fs::remove_dir_all(&tasks);
            std::fs::create_dir_all(&tasks)?;
            if is_virtual_account(&old.name) {
                deleted_virtual_profile =
                    account::delete_profile(&old.sid).unwrap_or_else(|error| {
                        eprintln!("The old service account's profile stays: {error:#}");
                        false
                    });
            } else if let Some(record) = &record
                && record.sid == old.sid
                && record.granted_service_logon
            {
                let _ = account::revoke_service_logon(&old.sid);
            }
        }
        match &target {
            Some(target) => {
                // Keep what an earlier install gave the same account.
                let earlier = record
                    .as_ref()
                    .is_some_and(|record| record.sid == new_sid && record.granted_service_logon);
                AccountRecord {
                    sid: target.account.sid.clone(),
                    granted_service_logon: granted || earlier,
                }
                .save(&data)?;
            }
            None => {
                let _ = std::fs::remove_file(data.join(AccountRecord::FILE));
            }
        }

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
        let runs_as = target
            .as_ref()
            .map(|target| target.account.name.clone())
            .unwrap_or(virtual_name);
        service.start::<&str>(&[]).map_err(|error| {
            if os_error(&error) == Some(ERROR_SERVICE_LOGON_FAILED) {
                anyhow::anyhow!(
                    "Windows could not log the service on as {runs_as}. If the account's \
                     password changed, run the command again with --new-password."
                )
            } else {
                anyhow::Error::new(error).context("starting the PSF Guard Worker service")
            }
        })?;
        Ok(Installed {
            account: runs_as,
            replaced: if changed {
                previous.map(|old| old.name)
            } else {
                None
            },
            granted_service_logon: granted,
            deleted_virtual_profile,
        })
    }

    pub fn uninstall() -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = open(
            &manager,
            ServiceAccess::QUERY_STATUS
                | ServiceAccess::QUERY_CONFIG
                | ServiceAccess::STOP
                | ServiceAccess::DELETE,
        )?
        .context("The PSF Guard Worker service is not installed")?;
        // Read the account while the service, and so a virtual account,
        // still exists.
        let ran_as = installed_account(&service);
        let record = AccountRecord::load(&data());
        stop_and_wait(&service)?;
        service.delete()?;
        drop(service);
        if let Some(ran_as) = &ran_as {
            if is_virtual_account(&ran_as.name) {
                if let Err(error) = account::delete_profile(&ran_as.sid) {
                    eprintln!("The service account's profile stays: {error:#}");
                }
            } else if let Some(record) = &record
                && record.sid == ran_as.sid
                && record.granted_service_logon
                && let Err(error) = account::revoke_service_logon(&ran_as.sid)
            {
                eprintln!(
                    "{} keeps the right to log on as a service: {error:#}",
                    ran_as.name
                );
            }
        }
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

    pub fn install(
        _settings: crate::commands::worker::WorkerSettings,
        _options: &InstallOptions,
    ) -> Result<Installed> {
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
    fn a_script_gives_the_password_on_one_line() {
        assert_eq!(
            read_password_line("pa ss\r\nnext\n".as_bytes()).unwrap(),
            "pa ss"
        );
        assert_eq!(read_password_line("secret".as_bytes()).unwrap(), "secret");
        assert!(read_password_line("\n".as_bytes()).is_err());
        assert!(read_password_line("".as_bytes()).is_err());
    }

    #[test]
    fn only_the_service_s_own_account_is_its_virtual_account() {
        assert!(is_virtual_account(r"NT SERVICE\PSFGuardWorker"));
        assert!(is_virtual_account(r"nt service\psfguardworker"));
        assert!(!is_virtual_account(r"PC\ann"));
        assert!(!is_virtual_account(r"NT SERVICE\OtherService"));
    }

    #[test]
    fn the_account_record_keeps_what_an_install_gave() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(AccountRecord::load(dir.path()), None);
        let record = AccountRecord {
            sid: "S-1-5-21-1-2-3-1001".into(),
            granted_service_logon: true,
        };
        record.save(dir.path()).unwrap();
        assert_eq!(AccountRecord::load(dir.path()), Some(record));
        // A record from a later version with more fields still loads.
        std::fs::write(
            dir.path().join(AccountRecord::FILE),
            r#"{"sid": "S-1-5-21-9", "later": 1}"#,
        )
        .unwrap();
        assert_eq!(
            AccountRecord::load(dir.path()),
            Some(AccountRecord {
                sid: "S-1-5-21-9".into(),
                granted_service_logon: false,
            })
        );
    }

    #[test]
    fn installing_needs_settings_that_can_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("worker.json");
        assert!(install(&missing, &InstallOptions::default())
            .unwrap_err()
            .to_string()
            .contains("Settings › Workers › Worker on this computer"));
        std::fs::write(
            &missing,
            r#"{"server": "observatory:3000", "token": "psfw_secret"}"#,
        )
        .unwrap();
        assert!(install(&missing, &InstallOptions::default())
            .unwrap_err()
            .to_string()
            .contains("http://"));
    }
}
