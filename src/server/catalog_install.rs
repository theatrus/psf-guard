//! Background installation of hosted Seiza catalog packages.
//!
//! Downloads are explicit and management-gated by the HTTP handler. The
//! installer uses Seiza's verified, content-addressed cache and materializes
//! the chosen package into the directory PSF Guard already searches.

use seiza_download::{CachePolicy, CatalogManager, CatalogSet, Dataset, DownloadEvent};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::server::state::InteractiveJobGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogInstallPreset {
    SolverLite,
    SolverGaia,
    BlindDeep,
    BlindDeepGaia20,
    /// Gaia DR3 colors for color calibration; an addition to a solver
    /// package, not a solver package itself.
    GaiaPhotometry,
}

impl CatalogInstallPreset {
    fn datasets(self) -> &'static [Dataset] {
        match self {
            Self::SolverLite => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsLiteTycho2,
                Dataset::StarsLiteTycho2Identifiers,
            ],
            Self::SolverGaia => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsGaia,
                Dataset::StarsLiteTycho2Identifiers,
            ],
            Self::BlindDeep => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsDeepGaia17,
                Dataset::BlindGaia16,
                Dataset::StarsLiteTycho2Identifiers,
            ],
            Self::BlindDeepGaia20 => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsDeepGaia20,
                Dataset::BlindGaia16,
                Dataset::StarsLiteTycho2Identifiers,
            ],
            Self::GaiaPhotometry => &[Dataset::GaiaPhotometry],
        }
    }

    fn selection(self) -> CatalogSet {
        self.datasets()
            .iter()
            .fold(CatalogSet::empty(), |set, dataset| set.with(*dataset))
    }

    fn file_count(self) -> usize {
        self.datasets().len()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogInstallRequest {
    pub preset: CatalogInstallPreset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogInstallPhase {
    Idle,
    Manifest,
    Downloading,
    Installing,
    Complete,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogInstallProgress {
    pub running: bool,
    pub phase: CatalogInstallPhase,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<CatalogInstallPreset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    pub files_completed: usize,
    pub files_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

impl Default for CatalogInstallProgress {
    fn default() -> Self {
        Self {
            running: false,
            phase: CatalogInstallPhase::Idle,
            message: "No catalog installation has run.".into(),
            preset: None,
            output_dir: None,
            file_name: None,
            files_completed: 0,
            files_total: 0,
            bytes_completed: None,
            bytes_total: None,
            written_bytes: None,
            installed_version: None,
            error: None,
            started_at: None,
            finished_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogInstallStatus {
    pub started: bool,
    pub progress: CatalogInstallProgress,
}

#[derive(Clone, Default)]
pub struct CatalogInstallManager {
    progress: Arc<RwLock<CatalogInstallProgress>>,
    /// The stop of the installation under way, or of the last one.
    stop: Arc<RwLock<crate::concurrency::Stop>>,
}

impl CatalogInstallManager {
    pub fn status(&self) -> CatalogInstallStatus {
        let progress = self.progress.read().unwrap().clone();
        CatalogInstallStatus {
            started: progress.running,
            progress,
        }
    }

    /// Start installing `preset` in the background and return at once. While
    /// one installation runs, another start returns its status without
    /// starting a second download. Stop, or the server stopping, ends it.
    pub fn start(
        &self,
        preset: CatalogInstallPreset,
        output_dir: PathBuf,
        guard: InteractiveJobGuard,
        shutdown: &Arc<crate::server::shutdown::Shutdown>,
    ) -> CatalogInstallStatus {
        let hold = shutdown.hold("a star catalog installation");
        let shutdown = Arc::clone(shutdown);
        self.start_with(
            preset,
            output_dir,
            (guard, hold),
            async move { shutdown.begun().await },
            |manager, preset, output_dir| async move { manager.install(preset, output_dir).await },
        )
    }

    /// Stop the installation under way. False when none runs.
    pub fn stop(&self) -> bool {
        if !self.progress.read().unwrap().running {
            return false;
        }
        self.stop.read().unwrap().request();
        true
    }

    /// [`Self::start`], with the work that installs, and what else ends
    /// it, given.
    fn start_with<G, E, F, Fut>(
        &self,
        preset: CatalogInstallPreset,
        output_dir: PathBuf,
        guard: G,
        server_stop: E,
        install: F,
    ) -> CatalogInstallStatus
    where
        G: Send + 'static,
        E: std::future::Future<Output = ()> + Send + 'static,
        F: FnOnce(Self, CatalogInstallPreset, PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        {
            let mut progress = self.progress.write().unwrap();
            if progress.running {
                return CatalogInstallStatus {
                    started: false,
                    progress: progress.clone(),
                };
            }
            *progress = CatalogInstallProgress {
                running: true,
                phase: CatalogInstallPhase::Manifest,
                message: "Checking the Seiza catalog manifest…".into(),
                preset: Some(preset),
                output_dir: Some(output_dir.to_string_lossy().into_owned()),
                file_name: None,
                files_completed: 0,
                files_total: preset.file_count(),
                bytes_completed: None,
                bytes_total: None,
                written_bytes: None,
                installed_version: None,
                error: None,
                started_at: Some(unix_now()),
                finished_at: None,
            };
        }

        let stop = crate::concurrency::Stop::new();
        *self.stop.write().unwrap() = stop.clone();
        let manager = self.clone();
        let work = install(self.clone(), preset, output_dir);
        tokio::spawn(async move {
            let _guard = guard;
            let ended = tokio::select! {
                ended = work => ended,
                () = stop.stopped() => Err("Stopped.".to_string()),
                () = server_stop => Err("The server stopped.".to_string()),
            };
            match ended {
                Ok(()) => {}
                Err(_) if stop.requested() => manager.finish_stopped(),
                Err(error) => manager.finish_with_error(error),
            }
        });

        CatalogInstallStatus {
            started: true,
            progress: self.progress.read().unwrap().clone(),
        }
    }

    async fn install(
        &self,
        preset: CatalogInstallPreset,
        output_dir: PathBuf,
    ) -> Result<(), String> {
        let catalog_manager = CatalogManager::builder()
            .policy(CachePolicy::ForceRefresh)
            .build()
            .map_err(|error| error.to_string())?;
        let downloaded = Arc::new(AtomicUsize::new(0));
        let progress = Arc::clone(&self.progress);
        let download_count = Arc::clone(&downloaded);
        let bundle = catalog_manager
            .ensure_with(&preset.selection(), move |event| {
                report_download_event(&progress, &download_count, event);
            })
            .await
            .map_err(|error| error.to_string())?;

        {
            let mut progress = self.progress.write().unwrap();
            progress.phase = CatalogInstallPhase::Installing;
            progress.message = format!("Installing catalog bundle {}…", bundle.version);
            progress.file_name = None;
            progress.files_completed = 0;
            progress.bytes_completed = None;
            progress.bytes_total = None;
            progress.written_bytes = None;
            progress.installed_version = Some(bundle.version.clone());
        }

        let installed = Arc::new(AtomicUsize::new(0));
        let progress = Arc::clone(&self.progress);
        let install_count = Arc::clone(&installed);
        bundle
            .materialize_with(&output_dir, move |event| {
                report_install_event(&progress, &install_count, event);
            })
            .await
            .map_err(|error| {
                format!(
                    "failed to install catalogs in {}: {error}",
                    output_dir.display()
                )
            })?;

        self.finish(&output_dir);
        Ok(())
    }

    fn finish(&self, output_dir: &std::path::Path) {
        let mut progress = self.progress.write().unwrap();
        progress.running = false;
        progress.phase = CatalogInstallPhase::Complete;
        progress.message = format!("Catalogs are ready in {}.", output_dir.display());
        progress.file_name = None;
        progress.files_completed = progress.files_total;
        progress.bytes_completed = None;
        progress.bytes_total = None;
        progress.written_bytes = None;
        progress.finished_at = Some(unix_now());
    }

    fn finish_stopped(&self) {
        tracing::info!("Seiza catalog installation stopped");
        let mut progress = self.progress.write().unwrap();
        progress.running = false;
        progress.phase = CatalogInstallPhase::Error;
        progress.message = "Catalog installation stopped. Catalogs installed before stay.".into();
        progress.error = Some("Stopped.".into());
        progress.finished_at = Some(unix_now());
    }

    fn finish_with_error(&self, error: String) {
        tracing::error!("Seiza catalog installation failed: {error}");
        let mut progress = self.progress.write().unwrap();
        progress.running = false;
        progress.phase = CatalogInstallPhase::Error;
        progress.message = format!("Catalog installation failed: {error}");
        progress.error = Some(error);
        progress.finished_at = Some(unix_now());
    }
}

fn report_download_event(
    progress: &RwLock<CatalogInstallProgress>,
    completed: &AtomicUsize,
    event: DownloadEvent,
) {
    let mut progress = progress.write().unwrap();
    match event {
        DownloadEvent::FetchingManifest { .. } => {
            progress.phase = CatalogInstallPhase::Manifest;
            progress.message = "Checking the Seiza catalog manifest…".into();
            progress.file_name = None;
        }
        DownloadEvent::UsingCachedManifest { version, stale } => {
            progress.phase = CatalogInstallPhase::Manifest;
            progress.message = if stale {
                format!("Using cached catalog manifest {version} while offline.")
            } else {
                format!("Using catalog manifest {version}.")
            };
            progress.file_name = None;
        }
        DownloadEvent::CacheHit { name, .. } => {
            progress.phase = CatalogInstallPhase::Downloading;
            progress.message = format!("Found {name} in the download cache.");
            progress.file_name = Some(name);
            progress.files_completed = completed.fetch_add(1, Ordering::Relaxed) + 1;
            clear_byte_progress(&mut progress);
        }
        DownloadEvent::DownloadStarted { name, bytes } => {
            progress.phase = CatalogInstallPhase::Downloading;
            progress.message = format!("Downloading {name}…");
            progress.file_name = Some(name);
            progress.bytes_completed = Some(0);
            progress.bytes_total = Some(bytes);
            progress.written_bytes = Some(0);
        }
        DownloadEvent::DownloadProgress {
            name,
            downloaded,
            total,
            written,
        } => {
            progress.phase = CatalogInstallPhase::Downloading;
            progress.message = format!("Downloading {name}…");
            progress.file_name = Some(name);
            progress.bytes_completed = Some(downloaded);
            progress.bytes_total = Some(total);
            progress.written_bytes = Some(written);
        }
        DownloadEvent::DownloadComplete { name, .. } => {
            progress.phase = CatalogInstallPhase::Downloading;
            progress.message = format!("Downloaded {name}.");
            progress.file_name = Some(name);
            progress.files_completed = completed.fetch_add(1, Ordering::Relaxed) + 1;
            clear_byte_progress(&mut progress);
        }
        DownloadEvent::Verifying { name } => {
            progress.phase = CatalogInstallPhase::Downloading;
            progress.message = format!("Verifying {name}…");
            progress.file_name = Some(name);
            clear_byte_progress(&mut progress);
        }
        DownloadEvent::Installing { .. } | DownloadEvent::InstallComplete { .. } => {}
    }
}

fn report_install_event(
    progress: &RwLock<CatalogInstallProgress>,
    completed: &AtomicUsize,
    event: DownloadEvent,
) {
    let mut progress = progress.write().unwrap();
    match event {
        DownloadEvent::Installing { name, .. } => {
            progress.phase = CatalogInstallPhase::Installing;
            progress.message = format!("Installing {name}…");
            progress.file_name = Some(name);
        }
        DownloadEvent::InstallComplete { name, .. } => {
            progress.phase = CatalogInstallPhase::Installing;
            progress.message = format!("Installed {name}.");
            progress.file_name = Some(name);
            progress.files_completed = completed.fetch_add(1, Ordering::Relaxed) + 1;
        }
        _ => {}
    }
}

fn clear_byte_progress(progress: &mut CatalogInstallProgress) {
    progress.bytes_completed = None;
    progress.bytes_total = None;
    progress.written_bytes = None;
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

    #[test]
    fn presets_request_the_expected_files() {
        let presets = [
            (CatalogInstallPreset::SolverLite, 5),
            (CatalogInstallPreset::SolverGaia, 5),
            (CatalogInstallPreset::BlindDeep, 6),
            (CatalogInstallPreset::BlindDeepGaia20, 6),
        ];

        for (preset, file_count) in presets {
            assert_eq!(preset.file_count(), file_count);
            assert!(!preset.datasets().contains(&Dataset::GaiaPhotometry));
            assert!(
                preset
                    .datasets()
                    .contains(&Dataset::StarsLiteTycho2Identifiers),
                "{preset:?} must install the star identifier catalog"
            );
        }
    }

    #[test]
    fn gaia_colors_install_alone() {
        assert_eq!(
            CatalogInstallPreset::GaiaPhotometry.datasets(),
            &[Dataset::GaiaPhotometry]
        );
        let preset: CatalogInstallPreset = serde_json::from_str("\"gaia_photometry\"").unwrap();
        assert_eq!(preset, CatalogInstallPreset::GaiaPhotometry);
    }

    /// Starting returns at once, while the download runs in the background
    /// and reports through the same status as every other package; a second
    /// start does not begin a second download.
    #[tokio::test]
    async fn an_install_returns_at_once_and_reports_progress_while_it_runs() {
        let manager = CatalogInstallManager::default();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let status = manager.start_with(
            CatalogInstallPreset::GaiaPhotometry,
            PathBuf::from("/catalogs"),
            (),
            std::future::pending(),
            |manager, _, output_dir| async move {
                report_download_event(
                    &manager.progress,
                    &AtomicUsize::new(0),
                    DownloadEvent::DownloadProgress {
                        name: "stars-gaia-photometry.bin".into(),
                        downloaded: 100,
                        total: 476,
                        written: 100,
                    },
                );
                released.await.map_err(|error| error.to_string())?;
                manager.finish(&output_dir);
                Ok(())
            },
        );
        assert!(status.started);
        assert!(status.progress.running);
        assert_eq!(
            status.progress.preset,
            Some(CatalogInstallPreset::GaiaPhotometry)
        );
        assert_eq!(status.progress.files_total, 1);

        let started_twice = manager.start_with(
            CatalogInstallPreset::BlindDeep,
            PathBuf::from("/catalogs"),
            (),
            std::future::pending(),
            |_, _, _| async { Err("a second download must not start".to_string()) },
        );
        assert!(!started_twice.started);
        assert_eq!(
            started_twice.progress.preset,
            Some(CatalogInstallPreset::GaiaPhotometry)
        );

        while manager.status().progress.bytes_completed.is_none() {
            tokio::task::yield_now().await;
        }
        let running = manager.status().progress;
        assert_eq!(running.phase, CatalogInstallPhase::Downloading);
        assert_eq!(
            (running.bytes_completed, running.bytes_total),
            (Some(100), Some(476))
        );

        release.send(()).unwrap();
        while manager.status().progress.running {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            manager.status().progress.phase,
            CatalogInstallPhase::Complete
        );
    }

    #[tokio::test]
    async fn stop_ends_an_installation_under_way() {
        let manager = CatalogInstallManager::default();
        assert!(!manager.stop(), "nothing runs");
        manager.start_with(
            CatalogInstallPreset::GaiaPhotometry,
            PathBuf::from("/catalogs"),
            (),
            std::future::pending(),
            |_, _, _| std::future::pending(),
        );
        assert!(manager.stop());
        while manager.status().progress.running {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let progress = manager.status().progress;
        assert_eq!(progress.phase, CatalogInstallPhase::Error);
        assert_eq!(progress.error.as_deref(), Some("Stopped."));
    }

    #[test]
    fn progress_events_track_bytes_and_completed_files() {
        let progress = RwLock::new(CatalogInstallProgress {
            running: true,
            files_total: 1,
            ..Default::default()
        });
        let completed = AtomicUsize::new(0);
        report_download_event(
            &progress,
            &completed,
            DownloadEvent::DownloadProgress {
                name: "objects.bin".into(),
                downloaded: 25,
                total: 100,
                written: 40,
            },
        );
        assert_eq!(progress.read().unwrap().bytes_completed, Some(25));

        report_download_event(
            &progress,
            &completed,
            DownloadEvent::DownloadComplete {
                name: "objects.bin".into(),
                path: PathBuf::from("/cache/objects.bin"),
            },
        );
        let progress = progress.read().unwrap();
        assert_eq!(progress.files_completed, 1);
        assert_eq!(progress.bytes_completed, None);
    }
}
