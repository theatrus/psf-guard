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
}

impl CatalogInstallPreset {
    fn datasets(self) -> &'static [Dataset] {
        match self {
            Self::SolverLite => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsLiteTycho2,
            ],
            Self::SolverGaia => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsGaia,
            ],
            Self::BlindDeep => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsDeepGaia17,
                Dataset::BlindGaia16,
            ],
            Self::BlindDeepGaia20 => &[
                Dataset::Objects,
                Dataset::MinorBodies,
                Dataset::Transients,
                Dataset::StarsDeepGaia20,
                Dataset::BlindGaia16,
            ],
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
}

impl CatalogInstallManager {
    pub fn status(&self) -> CatalogInstallStatus {
        let progress = self.progress.read().unwrap().clone();
        CatalogInstallStatus {
            started: progress.running,
            progress,
        }
    }

    pub fn start(
        &self,
        preset: CatalogInstallPreset,
        output_dir: PathBuf,
        guard: InteractiveJobGuard,
    ) -> CatalogInstallStatus {
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

        let manager = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(error) = manager.install(preset, output_dir).await {
                manager.finish_with_error(error);
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
        Ok(())
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
    fn presets_request_the_expected_number_of_files() {
        assert_eq!(CatalogInstallPreset::SolverLite.file_count(), 4);
        assert_eq!(CatalogInstallPreset::SolverGaia.file_count(), 4);
        assert_eq!(CatalogInstallPreset::BlindDeep.file_count(), 5);
        assert_eq!(CatalogInstallPreset::BlindDeepGaia20.file_count(), 5);
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
