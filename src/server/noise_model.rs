//! Seiza's noise model on the server: where its ONNX Runtime is, what is
//! installed, and the downloads a person starts in Settings. The provider
//! itself is in `remote_work::noise_model`.
//!
//! PSF Guard never fetches ONNX Runtime or the model by itself. Until a
//! person downloads both, or gives the path of an ONNX Runtime they
//! installed, the step is not offered and a request for it fails with what
//! to click.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::remote_work::noise_model::{self, install, Config, Installed};
use crate::server::api::ApiResponse;
use crate::server::handlers::{require_database_management_allowed, update_registry, AppError};
use crate::server::state::{AppState, InteractiveJobGuard};

/// The setting the registry stores.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoiseModelSettings {
    /// An ONNX Runtime a person installed: its library, or the folder that
    /// contains it. Absent uses the one PSF Guard downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onnxruntime: Option<String>,
}

static ONNXRUNTIME: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Use `settings` for every run and probe from now on. ONNX Runtime opens
/// once per process, so a path changed after a run needs a restart; the
/// capabilities say so.
pub fn configure(settings: Option<&NoiseModelSettings>) {
    *ONNXRUNTIME.write().unwrap_or_else(PoisonError::into_inner) = settings
        .and_then(|settings| settings.onnxruntime.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
}

/// Where this server's ONNX Runtime and model are, and whether the model
/// may use the GPU: Settings › GPU's main switch, as stacking reads it.
pub fn config() -> Config {
    Config::new(
        ONNXRUNTIME
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
        crate::server::stack_preview::gpu::current().enabled,
    )
}

/// What this server has installed for Seiza's models, and the download
/// under way: one part of `GET /api/tools/processing`.
#[derive(Debug, Clone, Serialize)]
pub struct SeizaInstall {
    /// ONNX Runtime and the noise model on this server.
    pub installed: Installed,
    /// The ONNX Runtime path a person set, when one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onnxruntime: Option<String>,
    /// The ONNX Runtime release PSF Guard downloads for this server, or
    /// absent when it has none for this kind of computer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_download: Option<String>,
    pub download: DownloadProgress,
}

impl SeizaInstall {
    /// Every viewer may see what is installed; folders and library paths
    /// on the server stay with the operator role.
    pub fn for_viewer(mut self) -> Self {
        self.installed.runtime_path = None;
        self.installed.folder = String::new();
        self.onnxruntime = None;
        self
    }
}

/// What is installed now. Opens ONNX Runtime when its library is on disk.
pub fn install_state(state: &AppState) -> SeizaInstall {
    SeizaInstall {
        installed: noise_model::SeizaEngine::new(config()).installed(),
        onnxruntime: config().onnxruntime.map(|path| path.display().to_string()),
        runtime_download: install::release().map(|_| install::ONNXRUNTIME_VERSION.to_string()),
        download: state.noise_model_downloads.progress(),
    }
}

/// PUT /api/settings/noise-model
///
/// The request gives a path on the server, so it needs database management.
pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<NoiseModelSettings>,
) -> Result<Json<ApiResponse<SeizaInstall>>, AppError> {
    require_database_management_allowed(&state)?;
    let settings = NoiseModelSettings {
        onnxruntime: request
            .onnxruntime
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty()),
    };
    if let Some(path) = &settings.onnxruntime {
        let given = Config {
            onnxruntime: Some(PathBuf::from(path)),
            ..config()
        };
        given.library().map_err(AppError::BadRequest)?;
    }
    let stored = (settings != NoiseModelSettings::default()).then(|| settings.clone());
    update_registry(&state, |registry| {
        registry.noise_model = stored;
        Ok(())
    })
    .await?;
    configure(Some(&settings));
    state.workers.forget_local_offers();
    let probed = Arc::clone(&state);
    let installed = tokio::task::spawn_blocking(move || install_state(&probed))
        .await
        .map_err(|error| AppError::InternalError(format!("Noise model probe failed: {error}")))?;
    Ok(Json(ApiResponse::success(installed)))
}

/// What a download fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPart {
    /// ONNX Runtime, from Microsoft's release.
    Runtime,
    /// The latest noise model Seiza publishes.
    Model,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadRequest {
    pub parts: Vec<DownloadPart>,
}

/// A download's state, for Settings and the job line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DownloadProgress {
    pub running: bool,
    /// The part being fetched now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<DownloadPart>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

/// The one download that may run at a time.
#[derive(Clone, Default)]
pub struct NoiseModelDownloads {
    progress: Arc<RwLock<DownloadProgress>>,
    /// The stop of the download under way, or of the last one.
    stop: Arc<RwLock<crate::concurrency::Stop>>,
}

impl NoiseModelDownloads {
    pub fn progress(&self) -> DownloadProgress {
        self.progress
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, change: impl FnOnce(&mut DownloadProgress)) {
        change(
            &mut self
                .progress
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// Stop the download under way. False when none runs.
    pub fn stop(&self) -> bool {
        if !self.progress().running {
            return false;
        }
        self.stop
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .request();
        true
    }

    /// Start fetching `parts` in the background and return at once. While
    /// one download runs, another start returns its progress without
    /// starting a second. Stop, or the server stopping, ends it.
    pub fn start(
        &self,
        parts: Vec<DownloadPart>,
        config: Config,
        guard: InteractiveJobGuard,
        state: &Arc<AppState>,
    ) -> DownloadProgress {
        let hold = state.shutdown.hold("a noise model download");
        let shutdown = Arc::clone(&state.shutdown);
        let workers = Arc::downgrade(state);
        self.start_with(
            parts,
            (guard, hold),
            async move { shutdown.begun().await },
            move |downloads, part| {
                let config = config.clone();
                async move {
                    let report = move |step: install::Step| {
                        downloads.set(|progress| {
                            progress.message = step.message;
                            progress.bytes_completed = step.bytes_done;
                            progress.bytes_total = step.bytes_total;
                        })
                    };
                    match part {
                        DownloadPart::Runtime => install::download_runtime(&config, &report)
                            .await
                            .map(|_| ()),
                        DownloadPart::Model => {
                            install::download_model(&config, &report).await.map(|_| ())
                        }
                    }
                }
            },
            move || {
                // The step is offered as soon as both parts are in place.
                if let Some(state) = workers.upgrade() {
                    state.workers.forget_local_offers();
                }
            },
        )
    }

    /// [`Self::start`], with the work that fetches one part, what else ends
    /// it, and what runs when it ends, given.
    fn start_with<G, E, F, Fut>(
        &self,
        parts: Vec<DownloadPart>,
        guard: G,
        server_stop: E,
        fetch: F,
        done: impl FnOnce() + Send + 'static,
    ) -> DownloadProgress
    where
        G: Send + 'static,
        E: std::future::Future<Output = ()> + Send + 'static,
        F: Fn(Self, DownloadPart) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        {
            let mut progress = self
                .progress
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if progress.running || parts.is_empty() {
                return progress.clone();
            }
            *progress = DownloadProgress {
                running: true,
                part: parts.first().copied(),
                message: "Starting…".into(),
                started_at: Some(unix_now()),
                ..Default::default()
            };
        }
        let stop = crate::concurrency::Stop::new();
        *self.stop.write().unwrap_or_else(PoisonError::into_inner) = stop.clone();
        let downloads = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let work = async {
                for part in parts {
                    downloads.set(|progress| {
                        progress.part = Some(part);
                        progress.bytes_completed = None;
                        progress.bytes_total = None;
                    });
                    fetch(downloads.clone(), part).await?;
                }
                Ok::<(), String>(())
            };
            let ended = tokio::select! {
                ended = work => ended,
                () = stop.stopped() => Err("Stopped".to_string()),
                () = server_stop => Err("The server stopped".to_string()),
            };
            downloads.set(|progress| {
                progress.running = false;
                progress.bytes_completed = None;
                progress.bytes_total = None;
                progress.finished_at = Some(unix_now());
                match ended {
                    Ok(()) => progress.message = "Downloaded".into(),
                    Err(error) => {
                        if !stop.requested() {
                            tracing::error!("Noise model download failed: {error}");
                        }
                        progress.message = if stop.requested() {
                            "Stopped. Nothing partly downloaded is used".into()
                        } else {
                            format!("The download failed: {error}")
                        };
                        progress.error = Some(error);
                    }
                }
            });
            done();
        });
        self.progress()
    }
}

/// POST /api/tools/noise-model/download
///
/// The download goes to the server's own folder, so it takes no path from
/// the client. The work continues after the request returns.
pub async fn start_download(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DownloadRequest>,
) -> Result<Json<ApiResponse<DownloadProgress>>, AppError> {
    let mut parts = Vec::new();
    for part in [DownloadPart::Runtime, DownloadPart::Model] {
        if request.parts.contains(&part) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Err(AppError::BadRequest(
            "Choose ONNX Runtime, the model, or both".into(),
        ));
    }
    if parts.contains(&DownloadPart::Runtime) && install::release().is_none() {
        return Err(AppError::BadRequest(install::NO_RELEASE.into()));
    }
    let guard = state.begin_named_interactive_job("a noise model download");
    let progress = state
        .noise_model_downloads
        .start(parts, config(), guard, &state);
    Ok(Json(ApiResponse::success(progress)))
}

/// POST /api/tools/noise-model/download/stop
pub async fn stop_download(State(state): State<Arc<AppState>>) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.noise_model_downloads.stop()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    async fn finished(downloads: &NoiseModelDownloads) -> DownloadProgress {
        for _ in 0..500 {
            let progress = downloads.progress();
            if !progress.running {
                return progress;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the download did not end");
    }

    #[tokio::test]
    async fn a_download_fetches_its_parts_in_order_and_runs_once() {
        let downloads = NoiseModelDownloads::default();
        let fetched = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(Mutex::new(false));
        let release = Arc::new(tokio::sync::Notify::new());
        let started = {
            let (fetched, release) = (Arc::clone(&fetched), Arc::clone(&release));
            let ended = Arc::clone(&ended);
            downloads.start_with(
                vec![DownloadPart::Runtime, DownloadPart::Model],
                (),
                std::future::pending(),
                move |downloads, part| {
                    let (fetched, release) = (Arc::clone(&fetched), Arc::clone(&release));
                    async move {
                        if part == DownloadPart::Runtime {
                            release.notified().await;
                        }
                        downloads.set(|progress| progress.message = format!("{part:?}"));
                        fetched.lock().unwrap().push(part);
                        Ok(())
                    }
                },
                move || *ended.lock().unwrap() = true,
            )
        };
        assert!(started.running);
        assert_eq!(started.part, Some(DownloadPart::Runtime));
        // A second start while one runs starts nothing.
        let again = downloads.start_with(
            vec![DownloadPart::Model],
            (),
            std::future::pending(),
            |_, _| async { Err("a second download ran".to_string()) },
            || {},
        );
        assert!(again.running);
        release.notify_one();
        let done = finished(&downloads).await;
        assert_eq!(done.error, None);
        assert_eq!(done.message, "Downloaded");
        assert_eq!(
            *fetched.lock().unwrap(),
            [DownloadPart::Runtime, DownloadPart::Model]
        );
        assert!(*ended.lock().unwrap());
    }

    #[tokio::test]
    async fn a_failed_part_ends_the_download_with_its_reason() {
        let downloads = NoiseModelDownloads::default();
        downloads.start_with(
            vec![DownloadPart::Runtime, DownloadPart::Model],
            (),
            std::future::pending(),
            |_, part| async move {
                match part {
                    DownloadPart::Runtime => Err("the archive does not match".to_string()),
                    DownloadPart::Model => panic!("the model was fetched after a failure"),
                }
            },
            || {},
        );
        let done = finished(&downloads).await;
        assert_eq!(done.error.as_deref(), Some("the archive does not match"));
        assert!(done.message.contains("failed"), "{}", done.message);
    }

    #[tokio::test]
    async fn stop_ends_a_download_and_reports_it_as_stopped() {
        let downloads = NoiseModelDownloads::default();
        assert!(!downloads.stop());
        downloads.start_with(
            vec![DownloadPart::Model],
            (),
            std::future::pending(),
            |_, _| std::future::pending(),
            || {},
        );
        assert!(downloads.stop());
        let done = finished(&downloads).await;
        assert_eq!(done.error.as_deref(), Some("Stopped"));
        assert!(done.message.starts_with("Stopped"), "{}", done.message);
    }

    #[test]
    fn a_viewer_sees_no_server_paths() {
        let install = SeizaInstall {
            installed: Installed {
                runtime_path: Some("/opt/onnx/libonnxruntime.so".into()),
                runtime_version: Some("1.31.1".into()),
                model: Some("noise-v1".into()),
                folder: "/srv/psf-guard/noise-model".into(),
                ..Default::default()
            },
            onnxruntime: Some("/opt/onnx".into()),
            runtime_download: None,
            download: DownloadProgress::default(),
        }
        .for_viewer();
        let json = serde_json::to_string(&install).unwrap();
        assert!(!json.contains("/opt/onnx"), "{json}");
        assert!(!json.contains("/srv/psf-guard"), "{json}");
        assert!(json.contains("1.31.1"));
    }
}
