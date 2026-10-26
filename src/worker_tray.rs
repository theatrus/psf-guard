//! `psf-guard worker --tray`: the worker in the macOS menu bar or the
//! Windows system tray. It runs with the settings the desktop app saved,
//! and serves its controls on a localhost port so the desktop app can show
//! its status, pause it and stop it. The process has no window; on macOS it
//! has no Dock icon either.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

use crate::commands::worker::control::{self, Connection, ControlClient, ControlFile};
use crate::commands::worker::{WorkerControl, WorkerLaunch, WorkerStatus};

/// How often the menu reads the worker's status.
const REFRESH: Duration = Duration::from_secs(2);
/// How long Quit waits for the worker to leave the server.
const QUIT_LIMIT: Duration = Duration::from_secs(35);

pub fn run(settings: PathBuf) -> Result<()> {
    init_logging();
    if already_running(&settings) {
        tracing::info!("A worker is already running in the menu bar or the system tray");
        return Ok(());
    }
    let control = WorkerControl::default();
    let worker = {
        let (control, settings) = (control.clone(), settings.clone());
        std::thread::Builder::new()
            .name("worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Runtime::new()?;
                let launch = WorkerLaunch::Settings(settings);
                let ran = runtime.block_on(crate::commands::worker::run_with(launch, control));
                if let Err(error) = &ran {
                    tracing::error!("The worker stopped: {error:#}");
                }
                ran
            })?
    };
    let controls = Controls::serve(control.clone(), ControlFile::path_for(&settings))?;

    let mut context = crate::tauri_main::context();
    // The tray has no window: the app's main window belongs to the app.
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .setup({
            let control = control.clone();
            move |app| {
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                build_tray(app.handle(), control, worker, controls)?;
                Ok(())
            }
        })
        .build(context)?
        .run(|_, event| match event {
            // With no window, nothing closing asks the tray to exit; only
            // Quit does.
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            // macOS counts this process as the running PSF Guard, since it
            // runs the app's executable, so opening the app from the Dock or
            // the Finder asks it to reopen. Open the app's window instead.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => open_app(),
            _ => {}
        });
    Ok(())
}

/// The tray's control port, served on a thread of its own so it outlives
/// the worker: the desktop app can still read why a worker stopped.
struct Controls {
    close: tokio::sync::oneshot::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Controls {
    fn serve(control: WorkerControl, file: PathBuf) -> Result<Self> {
        let (close, closed) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("worker-controls".into())
            .spawn(move || {
                let served = tokio::runtime::Runtime::new()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| {
                        runtime.block_on(control::serve_control(control, file, async {
                            let _ = closed.await;
                        }))
                    });
                if let Err(error) = served {
                    tracing::warn!("The worker's controls are not available: {error:#}");
                }
            })?;
        Ok(Self { close, thread })
    }

    /// Stop serving and remove the control file.
    fn close(self) {
        let _ = self.close.send(());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !self.thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Whether a worker answers on the controls this settings file's worker
/// would serve, so a second start leaves it alone.
fn already_running(settings: &Path) -> bool {
    let Some(client) = ControlClient::find(&ControlFile::path_for(settings)) else {
        return false;
    };
    tokio::runtime::Runtime::new()
        .map(|runtime| runtime.block_on(client.status()).is_ok())
        .unwrap_or(false)
}

fn build_tray(
    app: &tauri::AppHandle,
    control: WorkerControl,
    worker: std::thread::JoinHandle<Result<()>>,
    controls: Controls,
) -> tauri::Result<()> {
    let lines = MenuLines::of(&control.status());
    let connection = MenuItem::with_id(app, "connection", &lines.connection, false, None::<&str>)?;
    let work = MenuItem::with_id(app, "work", &lines.work, false, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", &lines.pause, true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open PSF Guard", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &connection,
            &work,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("worker")
        .tooltip(&lines.connection)
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    let tray = tray
        .on_menu_event({
            let control = control.clone();
            move |_, event| match event.id.as_ref() {
                "pause" => control.set_paused(!control.status().paused),
                "open" => open_app(),
                // The refresh below ends the tray once the worker has left
                // the server, which queues its running tasks again.
                "quit" => control.quit(),
                _ => {}
            }
        })
        .build(app)?;
    let app = app.clone();
    std::thread::spawn(move || {
        let mut shown = lines;
        let mut quit_since: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(REFRESH);
            // Quit, from the menu or the desktop app, ends the tray once the
            // worker has stopped, or after QUIT_LIMIT. A worker that stopped
            // on its own, such as one the server refused, keeps its tray, so
            // the menu and the desktop app can explain why.
            if control.quit_requested() {
                let since = *quit_since.get_or_insert_with(std::time::Instant::now);
                if worker.is_finished() || since.elapsed() >= QUIT_LIMIT {
                    controls.close();
                    app.exit(0);
                    return;
                }
            }
            let lines = MenuLines::of(&control.status());
            if lines == shown {
                continue;
            }
            let _ = connection.set_text(&lines.connection);
            let _ = work.set_text(&lines.work);
            let _ = pause.set_text(&lines.pause);
            let _ = tray.set_tooltip(Some(&lines.connection));
            shown = lines;
        }
    });
    Ok(())
}

/// Open the desktop app's window. The app's executable starts with no
/// arguments; when the app is open already, it brings that window forward
/// instead and exits.
fn open_app() {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(error) = std::process::Command::new(exe).spawn() {
                tracing::warn!("PSF Guard could not be opened: {error}");
            }
        }
        Err(error) => tracing::warn!("PSF Guard could not be found: {error}"),
    }
}

/// The menu's text for a worker's status.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuLines {
    connection: String,
    work: String,
    pause: String,
}

impl MenuLines {
    fn of(status: &WorkerStatus) -> Self {
        let connection = match status.connection {
            Connection::Connecting if status.server.is_empty() => "Starting".to_string(),
            Connection::Connecting => match &status.problem {
                Some(_) => format!("Cannot reach {}", status.server),
                None => format!("Connecting to {}", status.server),
            },
            Connection::Connected => format!("Connected to {}", status.server),
            Connection::Failed => format!(
                "Stopped: {}",
                status
                    .problem
                    .as_deref()
                    .unwrap_or("the worker could not continue")
            ),
            Connection::Stopped => "Stopped".to_string(),
        };
        let work = match status.tasks.as_slice() {
            [] if status.paused => "Paused".to_string(),
            [] => "Waiting for tasks".to_string(),
            [task] => format!("{}: {:.0}%", task.label, task.fraction * 100.0),
            tasks => format!("{} tasks running", tasks.len()),
        };
        let pause = if status.paused { "Resume" } else { "Pause" };
        Self {
            connection,
            work,
            pause: pause.into(),
        }
    }
}

/// Log to the console and to the app's log folder, in files of the
/// worker's own.
fn init_logging() {
    let filter = || {
        tracing_subscriber::filter::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info"))
    };
    let console = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(filter());
    let file = crate::desktop_log::log_dir()
        .and_then(|dir| crate::desktop_log::worker_file_appender(&dir).ok())
        .map(|appender| {
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(appender)
                .with_filter(filter())
        });
    let _ = tracing_subscriber::registry()
        .with(console)
        .with(file)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::worker::control::RunningTask;

    #[test]
    fn the_menu_explains_the_connection_the_work_and_the_pause() {
        let mut status = WorkerStatus {
            server: "http://observatory:3000".into(),
            name: "studio-pc".into(),
            ..Default::default()
        };
        let lines = MenuLines::of(&status);
        assert_eq!(lines.connection, "Connecting to http://observatory:3000");
        assert_eq!(lines.work, "Waiting for tasks");
        assert_eq!(lines.pause, "Pause");

        status.problem = Some("connection refused".into());
        assert_eq!(
            MenuLines::of(&status).connection,
            "Cannot reach http://observatory:3000"
        );

        status.connection = Connection::Connected;
        status.problem = None;
        status.tasks.push(RunningTask {
            id: "a".into(),
            label: "Stack of 30 frames, Ha channel".into(),
            fraction: 0.456,
        });
        let lines = MenuLines::of(&status);
        assert_eq!(lines.connection, "Connected to http://observatory:3000");
        assert_eq!(lines.work, "Stack of 30 frames, Ha channel: 46%");

        status.paused = true;
        status.tasks.clear();
        let lines = MenuLines::of(&status);
        assert_eq!(lines.work, "Paused");
        assert_eq!(lines.pause, "Resume");

        status.connection = Connection::Failed;
        status.problem = Some("The worker token was revoked".into());
        assert_eq!(
            MenuLines::of(&status).connection,
            "Stopped: The worker token was revoked"
        );
    }
}
