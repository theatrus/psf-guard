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
             Settings › Performance › Workers › Worker on this computer, or choose a file with \
             --config FILE",
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

/// The service's own virtual account, which Windows creates with it.
#[cfg_attr(not(windows), allow(dead_code))]
fn virtual_account_name() -> String {
    format!(r"NT SERVICE\{SERVICE_NAME}")
}

/// Whether a service account name is the service's own virtual account.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_virtual_account(name: &str) -> bool {
    name.eq_ignore_ascii_case(&virtual_account_name())
}

/// An account as `install` sees it: its SID, and its name for messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Seen {
    sid: String,
    name: String,
}

/// An account the service ran as before, whose access, task files and,
/// for the virtual account, profile an install still has to remove.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Leftover {
    sid: String,
    #[serde(default)]
    virtual_account: bool,
}

/// What `install` records in the service's data folder, before it changes
/// anything, so that a later install or uninstall can finish or undo it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
struct AccountRecord {
    /// Accounts an install gave "Log on as a service" and has not taken
    /// back.
    #[serde(default)]
    granted: Vec<String>,
    /// The previous account's clean-up, when an install stopped before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leftover: Option<Leftover>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl AccountRecord {
    const FILE: &'static str = "account.json";

    fn load(data: &Path) -> Self {
        std::fs::read_to_string(data.join(Self::FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, data: &Path) -> Result<()> {
        super::settings::write_private(&data.join(Self::FILE), &serde_json::to_vec_pretty(self)?)
    }

    /// The accounts whose granted right to take back once the service runs
    /// as `keep`.
    fn to_revoke(&self, keep: Option<&str>) -> Vec<String> {
        self.granted
            .iter()
            .filter(|sid| Some(sid.as_str()) != keep)
            .cloned()
            .collect()
    }
}

/// What `install` does about the account, decided before it changes
/// anything.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Plan {
    /// Ask for the new account's password.
    ask_password: bool,
    /// The account whose access, task files and profile to remove, from
    /// this change or an earlier install that stopped before it.
    leftover: Option<Leftover>,
    /// Clear a user account's stored password before the service moves to
    /// the virtual account: Windows keeps it otherwise.
    clear_password: bool,
}

/// Decide the account work. `previous` is the account the installed
/// service runs as, `target` the user account to run as, or `None` for
/// the virtual account.
#[cfg_attr(not(windows), allow(dead_code))]
fn plan(
    previous: Option<&Seen>,
    target: Option<&Seen>,
    record: &AccountRecord,
    password: Password,
) -> Plan {
    let previous_is_virtual = previous.is_some_and(|old| is_virtual_account(&old.name));
    let changed = match (previous, target) {
        (Some(old), Some(new)) => old.sid != new.sid,
        (Some(_), None) => !previous_is_virtual,
        (None, _) => false,
    };
    let ask_password = match target {
        Some(new) => {
            password != Password::WhenChanged || previous.is_none_or(|old| old.sid != new.sid)
        }
        None => false,
    };
    let leftover = if changed {
        previous.map(|old| Leftover {
            sid: old.sid.clone(),
            virtual_account: previous_is_virtual,
        })
    } else {
        record.leftover.clone()
    }
    // Moving back to the leftover account leaves nothing to remove.
    .filter(|leftover| target.is_none_or(|new| new.sid != leftover.sid))
    .filter(|leftover| target.is_some() || !leftover.virtual_account);
    Plan {
        ask_password,
        leftover,
        clear_password: target.is_none() && previous.is_some() && !previous_is_virtual,
    }
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

    /// The account the installed service runs as, or `None` when Windows
    /// cannot say or the account no longer exists.
    fn installed_account(service: &Service) -> Option<Seen> {
        let name = service.query_config().ok()?.account_name?;
        account::lookup(&name.to_string_lossy())
            .ok()
            .map(|found| Seen {
                sid: found.sid,
                name: found.name,
            })
    }

    fn ask_password(name: &str, how: Password) -> Result<String> {
        if how == Password::Stdin {
            return read_password_line(std::io::stdin().lock());
        }
        let password = rpassword::prompt_password(format!(
            "Windows password for {name} (the service logs on with it): "
        ))
        .context("reading the password; to give it from a script, use --password-stdin")?;
        anyhow::ensure!(!password.is_empty(), "No password was entered");
        Ok(password)
    }

    /// The service's settings, with an account and password when they
    /// change.
    fn service_info(
        binary: PathBuf,
        saved: &Path,
        account: Option<&str>,
        password: Option<&str>,
    ) -> ServiceInfo {
        ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: binary,
            launch_arguments: launch_arguments(saved),
            dependencies: Vec::new(),
            account_name: account.map(OsString::from),
            account_password: password.map(OsString::from),
        }
    }

    /// Remove what the previous account leaves: its task files, which the
    /// server queues again, and the virtual account's profile.
    fn clean_up(leftover: &Leftover, tasks: &Path) -> Result<bool> {
        let _ = std::fs::remove_dir_all(tasks);
        std::fs::create_dir_all(tasks)?;
        if !leftover.virtual_account {
            return Ok(false);
        }
        account::delete_profile(&leftover.sid)
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
        let mut record = AccountRecord::load(&data);
        let previous = existing.as_ref().and_then(installed_account);
        let virtual_name = virtual_account_name();

        // Settle the account, its right and its password while the old
        // service still runs, so a refused password changes nothing.
        let target = match &options.run_as {
            RunAs::Virtual => None,
            RunAs::Installer => Some(account::current()?),
            RunAs::Account(name) => Some(account::lookup(name)?),
        };
        if let Some(target) = &target {
            anyhow::ensure!(
                target.user,
                "{} is not a user account. Choose a person's account with --account, \
                 or install with --virtual-account.",
                target.name
            );
        }
        let seen = target.as_ref().map(|target| Seen {
            sid: target.sid.clone(),
            name: target.name.clone(),
        });
        let plan = plan(previous.as_ref(), seen.as_ref(), &record, options.password);
        let password = match &seen {
            Some(seen) if plan.ask_password => Some(ask_password(&seen.name, options.password)?),
            _ => None,
        };
        let mut granted = false;
        if let Some(target) = &target {
            granted = account::grant_service_logon(target)?;
            if let Some(password) = &password
                && let Err(error) = account::check_password(target, password)
            {
                if granted {
                    let _ = account::revoke_service_logon(&target.sid);
                }
                return Err(error);
            }
        }

        // Lock the folder before anything is written into it, then record
        // what this install gives and has to clean up, so that a retry after
        // a failure finishes the job.
        std::fs::create_dir_all(&data)?;
        let known = match (&target, &previous) {
            (Some(target), _) => Some(target.sid.clone()),
            (None, Some(old)) if is_virtual_account(&old.name) => Some(old.sid.clone()),
            (None, _) => None,
        };
        permissions::lock_down(&data, known.as_deref())?;
        if granted && let Some(target) = &target {
            record.granted.push(target.sid.clone());
        }
        record.leftover = plan.leftover.clone();
        record.save(&data)?;

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
            Some(target) => match &password {
                Some(password) => (Some(target.name.as_str()), Some(password.as_str())),
                None => (None, None),
            },
            None => (Some(virtual_name.as_str()), None),
        };
        let service = match existing {
            Some(service) => {
                if plan.clear_password {
                    // Windows keeps a user account's password with the
                    // service until another account with an empty password
                    // replaces it; a virtual account takes no password.
                    service.change_config(&service_info(
                        binary.clone(),
                        &saved,
                        Some(r"NT AUTHORITY\LocalService"),
                        Some(""),
                    ))?;
                }
                service.change_config(&service_info(
                    binary.clone(),
                    &saved,
                    account_name,
                    account_password,
                ))?;
                service
            }
            // A new service has no account yet, so a password was asked.
            None => manager
                .create_service(
                    &service_info(binary.clone(), &saved, account_name, account_password),
                    access,
                )
                .context("The service could not be created")?,
        };

        // Only now does a new virtual account exist to be granted access.
        let runs_as = match &target {
            Some(target) => Seen {
                sid: target.sid.clone(),
                name: target.name.clone(),
            },
            None => {
                let found = account::lookup(&virtual_name)?;
                Seen {
                    sid: found.sid,
                    name: found.name,
                }
            }
        };
        permissions::lock_down(&data, Some(&runs_as.sid))?;
        // What this leaves undone stays in the record for the next install.
        let mut deleted_virtual_profile = false;
        if let Some(leftover) = &record.leftover {
            match clean_up(leftover, &tasks) {
                Ok(deleted) => {
                    deleted_virtual_profile = deleted;
                    record.leftover = None;
                }
                Err(error) => eprintln!("The old service account's profile stays: {error:#}"),
            }
        }
        for sid in record.to_revoke(target.as_ref().map(|target| target.sid.as_str())) {
            match account::revoke_service_logon(&sid) {
                Ok(()) => record.granted.retain(|granted| *granted != sid),
                Err(error) => {
                    eprintln!("{sid} keeps the right to log on as a service: {error:#}")
                }
            }
        }
        record.save(&data)?;

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
        if let Err(error) = service.start::<&str>(&[]) {
            if os_error(&error) != Some(ERROR_SERVICE_LOGON_FAILED) {
                return Err(
                    anyhow::Error::new(error).context("starting the PSF Guard Worker service")
                );
            }
            // The stored password no longer works, such as after the
            // account's password changed: ask for the new one, rather than
            // leave the worker stopped.
            let target = match &target {
                Some(target) if password.is_none() => target,
                _ => anyhow::bail!(
                    "Windows could not log the service on as {}. If the account's password \
                     changed, run the command again with --new-password.",
                    runs_as.name
                ),
            };
            eprintln!(
                "Windows no longer accepts the password stored for {}.",
                target.name
            );
            let again = ask_password(&target.name, Password::Again)?;
            account::check_password(target, &again)?;
            service.change_config(&service_info(
                binary.clone(),
                &saved,
                Some(&target.name),
                Some(&again),
            ))?;
            service
                .start::<&str>(&[])
                .context("starting the PSF Guard Worker service")?;
        }
        drop(password);
        let replaced = previous
            .filter(|old| old.sid != runs_as.sid)
            .map(|old| old.name);
        Ok(Installed {
            account: runs_as.name,
            replaced,
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
        let profiles = ran_as
            .iter()
            .filter(|ran_as| is_virtual_account(&ran_as.name))
            .map(|ran_as| ran_as.sid.clone())
            .chain(
                record
                    .leftover
                    .iter()
                    .filter(|leftover| leftover.virtual_account)
                    .map(|leftover| leftover.sid.clone()),
            )
            .collect::<std::collections::BTreeSet<_>>();
        for sid in profiles {
            if let Err(error) = account::delete_profile(&sid) {
                eprintln!("The service account's profile stays: {error:#}");
            }
        }
        for sid in record.to_revoke(None) {
            if let Err(error) = account::revoke_service_logon(&sid) {
                eprintln!("{sid} keeps the right to log on as a service: {error:#}");
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
         systemd service. On macOS, open PSF Guard and choose Settings › Performance › Workers › Worker on this computer.";

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

    fn seen(sid: &str, name: &str) -> Seen {
        Seen {
            sid: sid.into(),
            name: name.into(),
        }
    }

    const ANN: &str = "S-1-5-21-1-2-3-1001";
    const BOB: &str = "S-1-5-21-1-2-3-1002";
    const SERVICE: &str = "S-1-5-80-1";

    #[test]
    fn moving_off_the_virtual_account_asks_for_a_password_and_cleans_up() {
        let old = seen(SERVICE, r"NT SERVICE\PSFGuardWorker");
        let ann = seen(ANN, r"PC\ann");
        let plan = plan(
            Some(&old),
            Some(&ann),
            &AccountRecord::default(),
            Password::WhenChanged,
        );
        assert_eq!(
            plan,
            Plan {
                ask_password: true,
                leftover: Some(Leftover {
                    sid: SERVICE.into(),
                    virtual_account: true,
                }),
                clear_password: false,
            }
        );
    }

    #[test]
    fn reinstalling_over_the_same_account_asks_only_when_told_to() {
        let ann = seen(ANN, r"PC\ann");
        let record = AccountRecord::default();
        let same = plan(Some(&ann), Some(&ann), &record, Password::WhenChanged);
        assert!(!same.ask_password);
        assert_eq!(same.leftover, None);
        assert!(plan(Some(&ann), Some(&ann), &record, Password::Again).ask_password);
        assert!(plan(Some(&ann), Some(&ann), &record, Password::Stdin).ask_password);
        // A new service has no stored password.
        assert!(plan(None, Some(&ann), &record, Password::WhenChanged).ask_password);
    }

    #[test]
    fn a_retry_finishes_the_clean_up_an_earlier_install_left() {
        // The first install moved the service to ann, then failed before
        // removing the virtual account's files and profile.
        let ann = seen(ANN, r"PC\ann");
        let record = AccountRecord {
            granted: vec![ANN.into()],
            leftover: Some(Leftover {
                sid: SERVICE.into(),
                virtual_account: true,
            }),
        };
        let plan = plan(Some(&ann), Some(&ann), &record, Password::WhenChanged);
        assert!(!plan.ask_password);
        assert_eq!(plan.leftover, record.leftover);
        // Its right stays: the service still runs as ann.
        assert!(record.to_revoke(Some(ANN)).is_empty());
    }

    #[test]
    fn moving_between_people_takes_back_only_what_an_install_gave() {
        let ann = seen(ANN, r"PC\ann");
        let bob = seen(BOB, r"PC\bob");
        let record = AccountRecord {
            granted: vec![ANN.into()],
            leftover: None,
        };
        let plan = plan(Some(&ann), Some(&bob), &record, Password::WhenChanged);
        assert!(plan.ask_password);
        assert_eq!(
            plan.leftover,
            Some(Leftover {
                sid: ANN.into(),
                virtual_account: false,
            })
        );
        assert_eq!(record.to_revoke(Some(BOB)), [ANN.to_string()]);
        // Someone switched the service to bob by hand; installing as bob
        // still takes back what an install gave ann.
        let by_hand = super::plan(Some(&bob), Some(&bob), &record, Password::WhenChanged);
        assert_eq!(by_hand.leftover, None);
        assert_eq!(record.to_revoke(Some(BOB)), [ANN.to_string()]);
    }

    #[test]
    fn moving_back_to_the_virtual_account_clears_the_stored_password() {
        let ann = seen(ANN, r"PC\ann");
        let plan = plan(
            Some(&ann),
            None,
            &AccountRecord::default(),
            Password::WhenChanged,
        );
        assert!(plan.clear_password);
        assert!(!plan.ask_password);
        assert_eq!(
            plan.leftover,
            Some(Leftover {
                sid: ANN.into(),
                virtual_account: false,
            })
        );
        // A leftover virtual account is the target again: its profile stays.
        let record = AccountRecord {
            granted: Vec::new(),
            leftover: Some(Leftover {
                sid: SERVICE.into(),
                virtual_account: true,
            }),
        };
        let old = seen(SERVICE, r"NT SERVICE\PSFGuardWorker");
        let back = super::plan(Some(&old), None, &record, Password::WhenChanged);
        assert_eq!(back.leftover, None);
        assert!(!back.clear_password);
    }

    #[test]
    fn an_account_that_cannot_be_found_leaves_nothing_to_clean_up() {
        let ann = seen(ANN, r"PC\ann");
        let plan = plan(
            None,
            Some(&ann),
            &AccountRecord::default(),
            Password::WhenChanged,
        );
        assert!(plan.ask_password);
        assert_eq!(plan.leftover, None);
    }

    #[test]
    fn the_account_record_survives_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(AccountRecord::load(dir.path()), AccountRecord::default());
        let record = AccountRecord {
            granted: vec![ANN.into()],
            leftover: Some(Leftover {
                sid: SERVICE.into(),
                virtual_account: true,
            }),
        };
        record.save(dir.path()).unwrap();
        assert_eq!(AccountRecord::load(dir.path()), record);
        // A damaged record loads as empty rather than stopping an install.
        std::fs::write(dir.path().join(AccountRecord::FILE), "{").unwrap();
        assert_eq!(AccountRecord::load(dir.path()), AccountRecord::default());
    }

    #[test]
    fn installing_needs_settings_that_can_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("worker.json");
        assert!(install(&missing, &InstallOptions::default())
            .unwrap_err()
            .to_string()
            .contains("Settings › Performance › Workers › Worker on this computer"));
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
