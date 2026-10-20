//! `GET /api/jobs`: every running, queued and waiting job on the server, as
//! uniform rows for the header's job list.
//!
//! The route reads the same in-memory state as each job's own status route:
//! catalog refreshes, quality scans and analyses, flat measuring, frame
//! sites, duplicate checks and repairs, reject removal, stack builds and
//! processing runs, automatic stack work waiting for the worker, WBPP runs,
//! collaboration transfers, Target Scheduler syncs and imports, backups and
//! catalog installs. It builds each row's title, scope and progress text on
//! the server and returns the rows in the order the header shows them: a
//! person's work first, then automatic work; within each, work that needs a
//! person, running work, each line's queued work in line order, then
//! waiting work.
//!
//! The route also lists each database's state for two client effects: the
//! header refreshes a database's views when its work finishes, and keeps a
//! note of a quality scan's frame errors once the scan ends.

use crate::server::job_text::{grouped, joined, percent, plural};
use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::Json;
use serde::Serialize;
use serde_json::Value;

use crate::server::api::ApiResponse;
use crate::server::auth::{AccessRole, RequestAccess};
use crate::server::backup_job::{JobStatus as BackupJob, Location};
use crate::server::calibration_duplicates::DuplicateJobStatus;
use crate::server::catalog_install::{CatalogInstallPreset, CatalogInstallProgress};
use crate::server::flat_measure::FlatMeasureStatus;
use crate::server::frame_sites::FrameSitesStatus;
use crate::server::previews::{PreviewKind, PreviewRun};
use crate::server::quality_backfill::QualityBackfillProgress;
use crate::server::reject_removal::RemovalJobStatus;
use crate::server::spatial_scan::SpatialScanProgress;
use crate::server::stack_preview::artifact::{ArtifactSearchJob, ArtifactSearchState};
use crate::server::stack_preview::{
    ScheduledRefreshView, StackActivity, StackActivityEntry, StackActivityKind, StackJobState,
    WaitingReapplyView,
};
use crate::server::state::{AppState, RefreshProgress, RefreshStage};
use crate::server::wbpp_run::{WbppActiveRun, WbppActivity, WbppQueuedActivity};

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    /// Stopped until a person acts, such as a transfer to review.
    Attention,
    /// At work now.
    Running,
    /// In a line a person can reorder.
    Queued,
    /// Paused for other work, or waiting for a retry.
    Waiting,
}

impl RowStatus {
    fn order(self) -> u32 {
        match self {
            Self::Attention => 0,
            Self::Running => 1,
            Self::Queued => 2,
            Self::Waiting => 3,
        }
    }
}

/// What kind of work a row lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    Refresh,
    Quality,
    Flats,
    Sites,
    Duplicates,
    Rejects,
    Stack,
    Processing,
    Wbpp,
    Transfer,
    Backup,
    Catalog,
    Grading,
    Health,
    Headers,
    FlatUse,
    Upkeep,
    Preview,
    Import,
    Export,
    ArtifactSearch,
    PlanningPush,
}

/// The two lines a person can reorder. Each runs one job at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowQueue {
    Stack,
    Wbpp,
}

impl RowQueue {
    fn order(self) -> u64 {
        match self {
            Self::Stack => 0,
            Self::Wbpp => 1,
        }
    }
}

/// The route a row's Stop or move buttons call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RowControl {
    Stack { job_id: String },
    Flats { db_id: String },
    Sites { db_id: String },
    Duplicates { db_id: String },
    Rejects { db_id: String },
    Backup,
    TsSync { job_id: String },
    WbppRunning { db_id: String },
    WbppQueued { db_id: String, queue_id: String },
    Preview { preview: PreviewKind, key: String },
    Import { db_id: String },
    Export { db_id: String },
    CatalogInstall,
}

/// A view in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkView {
    Grid,
    Sequence,
    Stacks,
    Calibration,
}

/// The view that shows what a job works on. The client builds the URL:
/// `/{view}?db=…&project=…&target=…`, then `params` in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowLink {
    pub view: LinkView,
    pub db: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<i32>,
    /// More query parameters, as name and value pairs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<(String, String)>,
}

impl RowLink {
    fn view(view: LinkView, db: &str) -> Self {
        Self {
            view,
            db: db.to_string(),
            project: None,
            target: None,
            params: Vec::new(),
        }
    }

    fn param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.params.push((name.to_string(), value.into()));
        self
    }
}

/// One job in the header's list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobRow {
    pub key: String,
    pub kind: RowKind,
    /// What is happening: "Stacking", "Analyzing quality".
    pub title: String,
    /// What it happens to: a database, or a target and channel.
    pub scope: String,
    /// The progress in words: "1/3 frames", a stage, a file.
    pub detail: String,
    pub status: RowStatus,
    /// Automatic or database-wide work, which runs after a person's work.
    pub background: bool,
    /// Work the server started rather than a person.
    pub automatic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// 0–100, or `null` when the job reports no progress. Waiting work
    /// keeps the progress it had reached.
    pub percent: Option<f64>,
    /// The work queued behind the current step, when the detail does not
    /// include it: "1 more rig to check", "3 channels to stack".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,
    /// The full path or file, for a tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<RowQueue>,
    /// Place among the queued jobs of its line, 0 next.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<RowControl>,
    /// Whether Stop can end the job while it runs.
    pub stoppable: bool,
    /// Text for the row's corner in place of the status, such as
    /// "Retry scheduled".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_label: Option<String>,
    /// The transfer job, as the transfers route lists it, for its controls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<RowLink>,
    /// When a running processing run started, Unix seconds. The client
    /// adds the local time to the detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
}

impl JobRow {
    fn new(
        key: impl Into<String>,
        kind: RowKind,
        title: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            kind,
            title: title.into(),
            scope: scope.into(),
            detail: String::new(),
            status: RowStatus::Running,
            background: false,
            automatic: false,
            waiting_for: None,
            percent: None,
            remaining: None,
            hint: None,
            queue: None,
            position: None,
            control: None,
            stoppable: false,
            state_label: None,
            transfer: None,
            link: None,
            started_at: None,
        }
    }

    /// Mark the row as automatic work in the automatic tier.
    fn automatic(mut self, automatic: bool) -> Self {
        self.automatic = automatic;
        self.background = automatic;
        self
    }
}

/// A quality scan's result, for the frame-error note the header shows once
/// the scan ends.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanResult {
    pub errors: usize,
    pub last_error: Option<String>,
    pub target_id: Option<i32>,
    pub project_id: Option<i32>,
}

/// One database's state, for the header's effects.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatabaseJobs {
    pub id: String,
    pub name: String,
    /// Whether work that changes the database's images, metrics or grades
    /// runs. When it turns false, the header refreshes the database's views.
    pub busy: bool,
    /// Whether a quality scan or analysis runs.
    pub scanning: bool,
    pub scan: ScanResult,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobsResponse {
    pub jobs: Vec<JobRow>,
    pub databases: Vec<DatabaseJobs>,
}

/// One database's job state, read from memory.
#[derive(Debug, Clone, Default)]
pub struct DatabaseState {
    pub id: String,
    pub name: String,
    /// The catalog refresh under way, if one is.
    pub refresh: Option<RefreshProgress>,
    pub scan: SpatialScanProgress,
    pub backfill: QualityBackfillProgress,
    pub flats: FlatMeasureStatus,
    pub sites: FrameSitesStatus,
    pub duplicates: DuplicateJobStatus,
    pub rejects: RemovalJobStatus,
    pub grading: crate::server::calibration_quality::CalibrationQualityProgress,
    pub health: crate::server::calibration_health::HealthProgress,
    /// The Calibration page's light header reads: lights read, and lights
    /// to read. `None` when none runs.
    pub header_reads: Option<(usize, usize)>,
    /// Whether what the flat groups feed is being worked out.
    pub flat_use: bool,
    pub upkeep: crate::server::catalog_upkeep::UpkeepStatus,
    /// An import that writes, while it runs. A preview is listed with the
    /// previews.
    pub import: Option<crate::server::import_job::ImportJobProgress>,
    /// An export into the export folder, while it runs.
    pub export: Option<crate::server::export_job::ExportJobProgress>,
}

/// A Target Scheduler sync or import job, as the rows need it.
#[derive(Debug, Clone, Default)]
pub struct TsSyncJob {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub stopping: bool,
    pub automatic: bool,
    pub source_connected: bool,
    pub stage: String,
    pub pages_done: u64,
    pub pages_total: u64,
    pub waiting_for: Option<String>,
    pub error: Option<String>,
}

impl From<crate::server::ts_sync::Job> for TsSyncJob {
    fn from(job: crate::server::ts_sync::Job) -> Self {
        Self {
            id: job.id,
            name: job.name,
            running: job.running,
            stopping: job.stopping,
            automatic: job.automatic,
            source_connected: job.source_connected,
            stage: job.stage,
            pages_done: job.pages_done,
            pages_total: job.pages_total,
            waiting_for: job.waiting_for,
            error: job.error,
        }
    }
}

/// Every job's state, read from memory.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub databases: Vec<DatabaseState>,
    pub stacks: StackActivity,
    pub wbpp: WbppActivity,
    pub transfers: Vec<Value>,
    pub ts_sync: Vec<TsSyncJob>,
    pub backup: Option<BackupJob>,
    pub catalog_install: Option<CatalogInstallProgress>,
    pub previews: Vec<PreviewRun>,
    /// Artifact searches in the stack line, with their places while they
    /// wait.
    pub artifact_searches: Vec<(ArtifactSearchJob, Option<usize>)>,
    /// Peers the director is resending planning to, by name.
    pub planning_pushes: Vec<String>,
    /// The person's work that background jobs wait for, if any runs.
    pub interactive: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            databases: Vec::new(),
            stacks: StackActivity {
                schema_version: 1,
                active: Vec::new(),
                scheduled: Vec::new(),
                waiting_reapplies: Vec::new(),
            },
            wbpp: WbppActivity {
                running: Vec::new(),
                queued: Vec::new(),
            },
            transfers: Vec::new(),
            ts_sync: Vec::new(),
            backup: None,
            catalog_install: None,
            previews: Vec::new(),
            artifact_searches: Vec::new(),
            planning_pushes: Vec::new(),
            interactive: None,
        }
    }
}

/// The project of a target in a database, or `None` when it is unknown.
pub type ProjectOf<'a> = &'a dyn Fn(&str, i32) -> Option<i32>;

// ---------------------------------------------------------------------------
// Text helpers, shared by several rows.

/// `Waiting for a stack build · 2/8 targets`: what the job waits for, then
/// the progress it had reached.
fn waiting_detail(waiting_for: Option<&str>, progress: &str) -> String {
    let waiting = match waiting_for {
        Some(what) if !what.is_empty() => format!("Waiting for {what}"),
        _ => "Waiting to start".to_string(),
    };
    joined([waiting.as_str(), progress])
}

/// The last part of a path: the folder or file name.
fn last_part(path: &str) -> &str {
    path.split(['/', '\\'])
        .rfind(|part| !part.is_empty())
        .unwrap_or(path)
}

/// A view at one target. When the target's project is unknown, the link
/// opens the database's images instead.
fn target_link(
    view: LinkView,
    db: &str,
    target: Option<i32>,
    project: Option<i32>,
    project_of: ProjectOf,
) -> RowLink {
    let project = project.or_else(|| target.and_then(|target| project_of(db, target)));
    match project {
        Some(project) => RowLink {
            project: Some(project),
            target,
            ..RowLink::view(view, db)
        },
        None => RowLink::view(LinkView::Grid, db),
    }
}

fn stacks_link(db: &str, project: i32, target: Option<i32>) -> RowLink {
    RowLink {
        project: Some(project),
        target,
        ..RowLink::view(LinkView::Stacks, db)
    }
}

// ---------------------------------------------------------------------------
// Rows for each kind of work.

fn refresh_stage_label(stage: &RefreshStage) -> &'static str {
    match stage {
        RefreshStage::Idle => "idle",
        RefreshStage::InitializingDirectoryTree => "Scanning folders",
        RefreshStage::LoadingProjects => "Loading projects",
        RefreshStage::ProcessingProjects => "Processing projects",
        RefreshStage::ProcessingTargets => "Processing targets",
        RefreshStage::UpdatingCache => "Updating cache",
        RefreshStage::Completed => "Finishing",
    }
}

fn refresh_row(db: &DatabaseState, refresh: &RefreshProgress) -> JobRow {
    let mut detail = match refresh.stage {
        RefreshStage::ProcessingProjects if refresh.projects_total > 0 => format!(
            "{}/{} projects",
            refresh.projects_processed, refresh.projects_total
        ),
        RefreshStage::ProcessingTargets if refresh.targets_total > 0 => format!(
            "{}/{} targets",
            refresh.targets_processed, refresh.targets_total
        ),
        RefreshStage::InitializingDirectoryTree => {
            let mut counts = Vec::new();
            if refresh.directories_processed > 0 {
                counts.push(format!("{} folders", refresh.directories_processed));
            }
            if refresh.files_scanned > 0 {
                counts.push(format!("{} files", refresh.files_scanned));
            }
            let counts = counts.join(", ");
            let place = refresh
                .current_directory_name
                .as_deref()
                .map(last_part)
                .unwrap_or("");
            joined(["Scanning folders", counts.as_str(), place])
        }
        _ => refresh_stage_label(&refresh.stage).to_string(),
    };
    if refresh.files_missing > 0 {
        detail.push_str(&format!(" · {} missing", refresh.files_missing));
    }
    let percent = refresh.get_progress_percentage() as f64;
    // A refresh the cache's expiry or the server's start began is
    // automatic; one a person requested with Refresh is theirs.
    JobRow {
        detail,
        percent: (percent > 0.0).then(|| percent.min(100.0)),
        hint: refresh
            .current_directory_name
            .clone()
            .or_else(|| refresh.current_project_name.clone()),
        link: Some(RowLink::view(LinkView::Grid, &db.id)),
        ..JobRow::new(
            format!("refresh:{}", db.id),
            RowKind::Refresh,
            "Refreshing catalog",
            &db.name,
        )
    }
    .automatic(!refresh.requested)
}

/// A database's quality work: a scan, a database-wide analysis, or both.
/// The analysis scans one target at a time, so its row includes the scan
/// under way. A scan a person started uses the database's scanner, and the
/// analysis waits for it in a row of its own.
fn quality_rows(db: &DatabaseState, project_of: ProjectOf) -> Vec<JobRow> {
    let scan = &db.scan;
    let backfill = &db.backfill;
    let verb = if scan.stage == "astrometry" {
        "Solving"
    } else {
        "Scanning"
    };
    // The target the scan is on: its sequence shows the scores it writes.
    let link = if scan.running {
        let mut link = target_link(LinkView::Sequence, &db.id, scan.target_id, None, project_of);
        if link.view == LinkView::Sequence
            && let Some(filter) = &scan.filter_name
        {
            link = link.param("filterName", filter.clone());
        }
        link
    } else {
        target_link(
            LinkView::Sequence,
            &db.id,
            backfill.current_target_id,
            None,
            project_of,
        )
    };
    let frames = if scan.stage == crate::server::spatial_scan::STAGE_PREPARING {
        "Finding frames to analyze".to_string()
    } else {
        format!("{verb} {}/{} frames", scan.processed, scan.total)
    };
    // An automatic scan paused for a person's work.
    let scan_waits_for = (scan.running && scan.automatic)
        .then(|| scan.waiting_for.clone())
        .flatten();
    let scan_row = |key: String| -> Option<JobRow> {
        scan.running.then(|| {
            JobRow {
                detail: match &scan_waits_for {
                    Some(what) => waiting_detail(Some(what.as_str()), &frames),
                    None => frames.clone(),
                },
                status: if scan_waits_for.is_some() {
                    RowStatus::Waiting
                } else {
                    RowStatus::Running
                },
                waiting_for: scan_waits_for.clone(),
                percent: percent(scan.processed, scan.total),
                hint: scan.current_file.clone(),
                link: Some(link.clone()),
                ..JobRow::new(key, RowKind::Quality, "Analyzing quality", &db.name)
            }
            .automatic(scan.automatic)
        })
    };
    if !backfill.running {
        return scan_row(format!("scan:{}", db.id)).into_iter().collect();
    }
    let targets = format!(
        "{}/{} targets",
        backfill.processed_targets, backfill.total_targets
    );
    let analysis = JobRow {
        detail: targets.clone(),
        percent: percent(backfill.processed_targets, backfill.total_targets),
        background: true,
        hint: scan.current_file.clone(),
        link: Some(link.clone()),
        ..JobRow::new(
            format!("backfill:{}", db.id),
            RowKind::Quality,
            "Analyzing database quality",
            &db.name,
        )
    };
    if scan.running && !scan.automatic {
        // A person's scan uses the scanner; the analysis waits for it.
        let waiting_for = "a quality scan";
        let person = scan_row(format!("scan:{}", db.id)).expect("the scan runs");
        return vec![
            person,
            JobRow {
                detail: waiting_detail(Some(waiting_for), &targets),
                status: RowStatus::Waiting,
                waiting_for: Some(waiting_for.into()),
                hint: None,
                link: Some(target_link(
                    LinkView::Sequence,
                    &db.id,
                    backfill.current_target_id,
                    None,
                    project_of,
                )),
                ..analysis
            },
        ];
    }
    if !scan.running {
        return vec![analysis];
    }
    let both = format!("{targets} · {frames}");
    vec![JobRow {
        detail: match &scan_waits_for {
            Some(what) => waiting_detail(Some(what.as_str()), &both),
            None => both,
        },
        status: if scan_waits_for.is_some() {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for: scan_waits_for,
        ..analysis
    }]
}

/// Flat measuring, once a pass has found work. Always automatic: a page
/// that shows the rig raises its priority, but no person started it.
fn flats_row(db: &DatabaseState, flats: &FlatMeasureStatus) -> JobRow {
    let mut link = RowLink::view(LinkView::Calibration, &db.id).param("calKind", "flat");
    if let Some(rig) = &flats.rig_uuid {
        link = link.param("calRig", rig.clone());
    }
    let label = crate::server::flat_measure::label(flats.stage.as_deref(), flats.done, flats.total);
    let scope = joined([db.name.as_str(), flats.rig_name.as_deref().unwrap_or("")]);
    JobRow {
        // While it pauses for a person's work it is listed as waiting, as
        // the other automatic jobs are, with the progress it had reached.
        detail: match &flats.waiting_for {
            Some(what) => waiting_detail(Some(what.as_str()), &label),
            None => label,
        },
        status: if flats.waiting_for.is_some() {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for: flats.waiting_for.clone(),
        percent: percent(flats.done, flats.total),
        remaining: crate::server::flat_measure::rigs_left(flats.rigs_after, &flats.next_pass),
        stoppable: true,
        control: Some(RowControl::Flats {
            db_id: db.id.clone(),
        }),
        link: Some(link),
        ..JobRow::new(
            format!("flats:{}", db.id),
            RowKind::Flats,
            "Measuring flats",
            scope,
        )
    }
    .automatic(true)
}

/// Reading the sites of calibration frames imported before the catalog
/// stored them, once a pass has frames to read. Automatic and stoppable.
fn sites_row(db: &DatabaseState, sites: &FrameSitesStatus) -> JobRow {
    let label = crate::server::frame_sites::label(sites.done, sites.total);
    JobRow {
        detail: match &sites.waiting_for {
            Some(what) => waiting_detail(Some(what.as_str()), &label),
            None => label,
        },
        status: if sites.waiting_for.is_some() {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for: sites.waiting_for.clone(),
        percent: percent(sites.done, sites.total),
        remaining: sites.next_pass.then(|| "another pass queued".to_string()),
        stoppable: true,
        control: Some(RowControl::Sites {
            db_id: db.id.clone(),
        }),
        link: Some(RowLink::view(LinkView::Calibration, &db.id)),
        ..JobRow::new(
            format!("sites:{}", db.id),
            RowKind::Sites,
            "Reading frame sites",
            &db.name,
        )
    }
    .automatic(true)
}

/// Calibration grading, once a pass has frames to measure: always
/// automatic. A pass that only regrades the library is quick, and gets no
/// row. While it waits for a person's work, the row shows what for.
fn grading_row(db: &DatabaseState, interactive: Option<&str>) -> JobRow {
    let grading = &db.grading;
    let waiting = grading.stage.as_deref() == Some("waiting");
    let label = match grading.stage.as_deref() {
        Some("grading") => "Grading the library".to_string(),
        _ => format!(
            "Measuring {} of {} {}",
            grouped(grading.done.min(grading.to_measure)),
            grouped(grading.to_measure),
            plural(grading.to_measure, "frame", "frames")
        ),
    };
    let waiting_for = waiting.then(|| interactive.map(str::to_string)).flatten();
    JobRow {
        detail: if waiting {
            waiting_detail(waiting_for.as_deref(), &label)
        } else {
            label
        },
        status: if waiting {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for,
        percent: percent(grading.done, grading.to_measure),
        link: Some(RowLink::view(LinkView::Calibration, &db.id)),
        ..JobRow::new(
            format!("grading:{}", db.id),
            RowKind::Grading,
            "Grading calibration frames",
            &db.name,
        )
    }
    .automatic(true)
}

/// An import that writes. A scheduled one is automatic work.
fn import_row(db: &DatabaseState, import: &crate::server::import_job::ImportJobProgress) -> JobRow {
    let automatic = import.trigger == "automatic";
    let detail = match import.stage.as_str() {
        "importing" => "Writing frames to the catalog".to_string(),
        _ => format!(
            "{} of {} {} read",
            grouped(import.scanned_files.min(import.total_files)),
            grouped(import.total_files),
            plural(import.total_files, "header", "headers")
        ),
    };
    JobRow {
        detail,
        percent: (import.stage == "scanning")
            .then(|| percent(import.scanned_files, import.total_files))
            .flatten(),
        background: automatic,
        automatic,
        control: Some(RowControl::Import {
            db_id: db.id.clone(),
        }),
        stoppable: true,
        ..JobRow::new(
            format!("import:{}", db.id),
            RowKind::Import,
            if automatic {
                "Importing new frames"
            } else {
                "Importing frames"
            },
            &db.name,
        )
    }
}

/// An export into the database's export folder.
fn export_row(db: &DatabaseState, export: &crate::server::export_job::ExportJobProgress) -> JobRow {
    let detail = match export.stage.as_str() {
        "placing" => format!(
            "{} of {} {} placed",
            grouped(export.placed_files.min(export.total_files)),
            grouped(export.total_files),
            plural(export.total_files, "file", "files")
        ),
        "scripts" => "Writing the WBPP runner".to_string(),
        _ => "Planning the export".to_string(),
    };
    JobRow {
        detail,
        percent: (export.stage == "placing")
            .then(|| percent(export.placed_files, export.total_files))
            .flatten(),
        hint: Some(export.destination.clone()),
        control: Some(RowControl::Export {
            db_id: db.id.clone(),
        }),
        stoppable: true,
        ..JobRow::new(
            format!("export:{}", db.id),
            RowKind::Export,
            if export.scope.is_empty() {
                "Exporting frames".to_string()
            } else {
                format!("Exporting {}", export.scope)
            },
            &db.name,
        )
    }
}

/// The catalog's upkeep while it runs: building PSF Guard's query indexes,
/// reading the readout modes of calibration frames that lack them, or
/// pairing calibrated copies with their lights. Always automatic. Reading
/// readout modes gets a row only once it has frames to read.
fn upkeep_row(db: &DatabaseState) -> Option<JobRow> {
    use crate::server::catalog_upkeep::UpkeepStage;
    let upkeep = &db.upkeep;
    let (title, detail) = match upkeep.stage {
        UpkeepStage::Idle => return None,
        UpkeepStage::BuildingIndexes => ("Building query indexes", String::new()),
        UpkeepStage::ReadingReadoutModes if upkeep.total == 0 => return None,
        UpkeepStage::ReadingReadoutModes => (
            "Reading readout modes",
            format!(
                "{} of {} calibration {}",
                grouped(upkeep.done.min(upkeep.total)),
                grouped(upkeep.total),
                plural(upkeep.total, "frame", "frames")
            ),
        ),
        UpkeepStage::PairingCopies => ("Pairing calibrated copies", String::new()),
    };
    Some(
        JobRow {
            detail,
            status: RowStatus::Running,
            percent: percent(upkeep.done, upkeep.total),
            link: Some(RowLink::view(LinkView::Calibration, &db.id)),
            ..JobRow::new(
                format!("upkeep:{}", db.id),
                RowKind::Upkeep,
                title,
                &db.name,
            )
        }
        .automatic(true),
    )
}

/// Working out calibration health, once a pass has projects to judge:
/// always automatic.
fn health_row(db: &DatabaseState) -> JobRow {
    let health = &db.health;
    JobRow {
        detail: format!(
            "{} of {} {}",
            grouped(health.judged.min(health.total)),
            grouped(health.total),
            plural(health.total, "project", "projects")
        ),
        percent: percent(health.judged, health.total),
        link: Some(RowLink::view(LinkView::Calibration, &db.id)),
        ..JobRow::new(
            format!("health:{}", db.id),
            RowKind::Health,
            "Checking calibration health",
            &db.name,
        )
    }
    .automatic(true)
}

/// The Calibration page reading light headers the health pass has not kept:
/// a view asked for them, so they are a person's work.
fn headers_row(db: &DatabaseState, (read, total): (usize, usize)) -> JobRow {
    JobRow {
        detail: format!(
            "{} of {} {}",
            grouped(read.min(total)),
            grouped(total),
            plural(total, "header", "headers")
        ),
        percent: percent(read, total),
        link: Some(RowLink::view(LinkView::Calibration, &db.id)),
        ..JobRow::new(
            format!("headers:{}", db.id),
            RowKind::Headers,
            "Reading light headers",
            &db.name,
        )
    }
}

/// Working out what the flat groups feed, for the Calibration page and the
/// Images filter: background work.
fn flat_use_row(db: &DatabaseState) -> JobRow {
    JobRow {
        detail: "Matching lights to flat groups".into(),
        link: Some(RowLink::view(LinkView::Calibration, &db.id)),
        ..JobRow::new(
            format!("flat-use:{}", db.id),
            RowKind::FlatUse,
            "Working out what the flat groups feed",
            &db.name,
        )
    }
    .automatic(true)
}

/// The duplicate check or repair: a person's, or the check a page starts on
/// its own, which runs as background work and is listed as waiting while it
/// pauses for a person's work.
fn duplicates_row(db: &DatabaseState, duplicates: &DuplicateJobStatus) -> JobRow {
    use crate::server::calibration_duplicates as duplicate_job;
    let label = duplicate_job::label(
        duplicates.stage.as_deref(),
        duplicates.done,
        duplicates.total,
    );
    let background = duplicates.priority.as_deref() == Some("background");
    let title = duplicate_job::title(duplicates.action.as_deref());
    JobRow {
        detail: match &duplicates.waiting_for {
            Some(what) => waiting_detail(Some(what.as_str()), &label),
            None => label,
        },
        status: if duplicates.waiting_for.is_some() {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for: duplicates.waiting_for.clone(),
        percent: percent(duplicates.done, duplicates.total),
        stoppable: true,
        control: Some(RowControl::Duplicates {
            db_id: db.id.clone(),
        }),
        link: Some(RowLink::view(LinkView::Calibration, &db.id).param("calDupes", "true")),
        ..JobRow::new(
            format!("duplicates:{}", db.id),
            RowKind::Duplicates,
            title,
            &db.name,
        )
    }
    .automatic(background)
}

/// Purge Rejects' preview, removal, restore or trash emptying. A person
/// starts each from Settings › Databases, which is a dialog, so the row has
/// no link.
fn rejects_row(db: &DatabaseState, rejects: &RemovalJobStatus) -> JobRow {
    use crate::server::reject_removal as removal_job;
    let title = removal_job::title(rejects.action.as_deref());
    JobRow {
        detail: removal_job::label(rejects.stage.as_deref(), rejects.done, rejects.total),
        percent: percent(rejects.done, rejects.total),
        stoppable: true,
        control: Some(RowControl::Rejects {
            db_id: db.id.clone(),
        }),
        ..JobRow::new(
            format!("rejects:{}", db.id),
            RowKind::Rejects,
            title,
            &db.name,
        )
    }
}

/// A stretch, deconvolution or RC-Astro run on a finished stack. It waits
/// in the same line as builds, so it moves and leaves the line the same way.
fn processing_row(entry: &StackActivityEntry) -> JobRow {
    let queued = entry.state == StackJobState::Queued;
    let processing = entry.processing.as_ref();
    let operation = processing.map(|run| run.operation.as_str());
    let waiting_for = processing.and_then(|run| run.waiting_for.as_deref());
    // The title is the operation. The detail is what a queued run waits
    // for, or the step under way when the title does not already show it;
    // the client adds when the run started, since a tool can take many
    // minutes.
    let started_at = processing
        .and_then(|run| run.started_unix_seconds)
        .filter(|_| !queued);
    let step = processing
        .and_then(|run| run.stage.as_deref())
        .filter(|stage| Some(*stage) != operation)
        .unwrap_or("");
    let detail = if queued {
        match waiting_for {
            Some(what) => format!("Waiting for {what}"),
            None => entry.detail.clone(),
        }
    } else if step.is_empty() && started_at.is_none() {
        entry.detail.clone()
    } else {
        step.to_string()
    };
    JobRow {
        detail,
        status: if queued {
            RowStatus::Queued
        } else {
            RowStatus::Running
        },
        percent: processing
            .and_then(|run| run.fraction)
            .filter(|_| !queued)
            .map(|fraction| (f64::from(fraction) * 100.0).min(100.0)),
        hint: Some(match waiting_for {
            Some(what) => format!(
                "{} on {}, waiting for {what}",
                operation.unwrap_or("Processing"),
                entry.label
            ),
            None => entry.detail.clone(),
        }),
        queue: Some(RowQueue::Stack),
        position: if queued { entry.queue_position } else { None },
        control: Some(RowControl::Stack {
            job_id: entry.job_id.clone(),
        }),
        // A running stretch or deconvolution finishes in moments; an
        // RC-Astro tool can be stopped.
        stoppable: queued || processing.is_some_and(|run| run.stoppable),
        // The channel's card is on its target's stacks.
        link: Some(stacks_link(
            &entry.database_id,
            entry.project_id,
            entry.target_id,
        )),
        started_at,
        ..JobRow::new(
            format!("stack:{}", entry.job_id),
            RowKind::Processing,
            operation.unwrap_or("Processing"),
            &entry.label,
        )
    }
    // Re-applied by the server after its stack refreshed.
    .automatic(entry.automatic)
}

fn stack_row(entry: &StackActivityEntry) -> JobRow {
    if entry.kind == StackActivityKind::Processing {
        return processing_row(entry);
    }
    let mono = entry.kind == StackActivityKind::Mono;
    let queued = entry.state == StackJobState::Queued;
    // A build being prepared is in line while its frames and calibration
    // are read; it becomes the builds it was prepared into. A composition
    // being prepared may wait for its channel builds first.
    let title = match (entry.state == StackJobState::Preparing, mono) {
        (true, false) => "Preparing color preview",
        (true, true) => "Preparing stack build",
        (false, true) => "Stacking",
        (false, false) => "Composing color",
    };
    let unit = if mono { "frames" } else { "steps" };
    let detail = if queued {
        // A composition with no place in line waits in the quick lane,
        // behind another composition rather than a build.
        if !mono && entry.queue_position.is_none() {
            "Waiting for the composition ahead".to_string()
        } else {
            "Waiting for the build ahead".to_string()
        }
    } else if let Some(label) = &entry.progress_label {
        label.clone()
    } else if entry.total_units > 0 {
        format!("{}/{} {unit}", entry.processed_units, entry.total_units)
    } else {
        entry.detail.clone()
    };
    JobRow {
        detail,
        status: if queued {
            RowStatus::Queued
        } else {
            RowStatus::Running
        },
        percent: if queued {
            None
        } else {
            percent(entry.processed_units, entry.total_units)
        },
        hint: Some(entry.detail.clone()),
        queue: Some(RowQueue::Stack),
        position: if queued { entry.queue_position } else { None },
        control: Some(RowControl::Stack {
            job_id: entry.job_id.clone(),
        }),
        // A build stops at its next frame; a color composition at its next
        // channel or step.
        stoppable: true,
        link: Some(stacks_link(
            &entry.database_id,
            entry.project_id,
            entry.target_id,
        )),
        ..JobRow::new(
            format!("stack:{}", entry.job_id),
            RowKind::Stack,
            title,
            &entry.label,
        )
    }
    .automatic(entry.automatic)
}

fn wbpp_stage_label(stage: &str) -> &str {
    match stage {
        "planning" => "Planning the run",
        "running" => "Running in PixInsight",
        "publishing" => "Copying masters",
        other => other,
    }
}

fn wbpp_running_row(run: &WbppActiveRun, project_of: ProjectOf) -> JobRow {
    let publishing = run.stage == "publishing";
    let steps = if run.wbpp_steps > 0 {
        format!("{} steps done", run.wbpp_steps)
    } else {
        String::new()
    };
    JobRow {
        detail: joined([
            wbpp_stage_label(&run.stage),
            run.wbpp_stage.as_deref().unwrap_or(""),
            steps.as_str(),
        ]),
        queue: Some(RowQueue::Wbpp),
        // A masters copy is not PixInsight's work: Stop does not end it.
        stoppable: !publishing,
        control: (!publishing).then(|| RowControl::WbppRunning {
            db_id: run.db_id.clone(),
        }),
        // The stacks view shows a project's WBPP run and its outputs.
        link: Some(target_link(
            LinkView::Stacks,
            &run.db_id,
            run.target_id,
            run.project_id,
            project_of,
        )),
        // The start time is part of the key, so the next run on the same
        // database gets a new row, without the previous row's Stop
        // confirmation.
        ..JobRow::new(
            format!(
                "wbpp:{}:{}",
                run.db_id,
                run.started_at.map(|at| at.to_string()).unwrap_or_default()
            ),
            RowKind::Wbpp,
            "WBPP",
            format!("{} · {}", run.db_name, run.scope),
        )
    }
}

fn wbpp_queued_row(run: &WbppQueuedActivity, index: usize, project_of: ProjectOf) -> JobRow {
    JobRow {
        detail: "Waiting for the run ahead".into(),
        status: RowStatus::Queued,
        queue: Some(RowQueue::Wbpp),
        position: Some(index),
        stoppable: true,
        control: Some(RowControl::WbppQueued {
            db_id: run.db_id.clone(),
            queue_id: run.run.id.clone(),
        }),
        link: Some(target_link(
            LinkView::Stacks,
            &run.db_id,
            run.run.target_id,
            run.run.project_id,
            project_of,
        )),
        ..JobRow::new(
            format!("wbpp-queued:{}", run.run.id),
            RowKind::Wbpp,
            "WBPP",
            format!("{} · {}", run.db_name, run.run.scope),
        )
    }
}

/// An automatic refresh that came due and waits for a quality analysis of
/// its database before its builds join the line.
fn waiting_refresh_row(refresh: &ScheduledRefreshView) -> JobRow {
    let project = match (refresh.project_id, &refresh.project_name) {
        (None, _) => "every followed project".to_string(),
        (Some(_), Some(name)) => name.clone(),
        (Some(id), None) => format!("project {id}"),
    };
    let channels = refresh.channels.len();
    JobRow {
        detail: waiting_detail(refresh.waiting_for.as_deref(), ""),
        status: RowStatus::Waiting,
        waiting_for: refresh.waiting_for.clone(),
        percent: None,
        remaining: (channels > 0).then(|| {
            format!(
                "{channels} {} to stack",
                plural(channels, "channel", "channels")
            )
        }),
        hint: (channels > 0).then(|| format!("Restacks {}", refresh.channels.join(", "))),
        link: Some(match refresh.project_id {
            None => RowLink::view(LinkView::Grid, &refresh.database_id),
            Some(project) => stacks_link(&refresh.database_id, project, None),
        }),
        ..JobRow::new(
            format!(
                "refresh-waiting:{}:{}",
                refresh.database_id,
                refresh
                    .project_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "all".into())
            ),
            RowKind::Stack,
            "Refreshing stacks",
            format!("{} · {project}", refresh.database_name),
        )
    }
    .automatic(true)
}

/// A re-apply waiting for its turn: automatic after a refresh, the
/// person's own after a build they started.
fn waiting_reapply_row(reapply: &WaitingReapplyView) -> JobRow {
    JobRow {
        detail: waiting_detail(reapply.waiting_for.as_deref(), ""),
        status: RowStatus::Waiting,
        waiting_for: reapply.waiting_for.clone(),
        link: Some(stacks_link(
            &reapply.database_id,
            reapply.project_id,
            reapply.target_id,
        )),
        ..JobRow::new(
            format!(
                "reapply-waiting:{}:{}:{}",
                reapply.database_id, reapply.project_id, reapply.label
            ),
            RowKind::Processing,
            "Re-applying processing",
            &reapply.label,
        )
    }
    .automatic(!reapply.personal)
}

/// A collaboration transfer: a person's, or one the connection's policy
/// queued. A transfer held for a person's review, before upload or after a
/// failed upload, needs attention; a paused transfer, or one waiting for
/// its retry, is listed as waiting. Returns `None` for a finished transfer.
fn transfer_row(job: &Value) -> Option<JobRow> {
    let text = |field: &str| job[field].as_str().unwrap_or("").to_string();
    let number = |field: &str| job[field].as_u64().unwrap_or(0);
    let state = text("state");
    if matches!(state.as_str(), "complete" | "cancelled") {
        return None;
    }
    let active = matches!(state.as_str(), "uploading" | "preparing");
    let sent = number("completed_bytes") + number("sent_bytes");
    let total_bytes = number("total_bytes");
    let state_label = match state.as_str() {
        "preparing" => "Checking files",
        "review" => "Review required",
        "queued" => "Pending",
        "uploading" => "Uploading",
        "paused" => "Paused",
        "retry" => "Retry scheduled",
        "needs_review" => "Needs attention",
        _ => "",
    };
    let current_file = text("current_file");
    let automatic = job["automatic"].as_bool().unwrap_or(false);
    Some(
        JobRow {
            detail: job["error"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "{}/{} files · {:.1} MiB",
                        number("completed_files"),
                        number("total_files"),
                        sent as f64 / 1024.0 / 1024.0
                    )
                }),
            status: if active {
                RowStatus::Running
            } else if matches!(state.as_str(), "review" | "needs_review") {
                RowStatus::Attention
            } else if state == "queued" {
                RowStatus::Queued
            } else {
                RowStatus::Waiting
            },
            state_label: (!state_label.is_empty()).then(|| state_label.to_string()),
            percent: (active && total_bytes > 0)
                .then(|| (100.0 * sent as f64 / total_bytes as f64).min(100.0)),
            hint: (!current_file.is_empty()).then_some(current_file),
            transfer: Some(job.clone()),
            ..JobRow::new(
                format!("transfer:{}", text("id")),
                RowKind::Transfer,
                "Transfers",
                format!("{} · {}", text("catalog"), text("label")),
            )
        }
        .automatic(automatic),
    )
}

/// A Target Scheduler sync or import that runs, or that failed. Returns
/// `None` for one that finished without an error.
fn ts_sync_row(job: &TsSyncJob) -> Option<JobRow> {
    if !job.running && job.error.is_none() {
        return None;
    }
    let title = if job.id.starts_with("import:") {
        if job.source_connected {
            "Importing Target Scheduler"
        } else {
            "Importing catalog copy"
        }
    } else {
        "Target Scheduler sync"
    };
    let detail = match (&job.error, job.stage.as_str()) {
        (Some(error), _) if !error.is_empty() => error.clone(),
        (_, "copying") => "Copying a consistent database snapshot".into(),
        (_, "initializing") => "Preparing the working catalog".into(),
        _ => "Importing captures and syncing grades".into(),
    };
    Some(
        JobRow {
            detail,
            status: if job.error.is_some() {
                RowStatus::Attention
            } else if job.waiting_for.is_some() {
                RowStatus::Waiting
            } else {
                RowStatus::Running
            },
            waiting_for: job.waiting_for.clone(),
            control: job.running.then(|| RowControl::TsSync {
                job_id: job.id.clone(),
            }),
            stoppable: job.running && !job.stopping,
            percent: (job.stage == "copying" && job.pages_total > 0)
                .then(|| (100.0 * job.pages_done as f64 / job.pages_total as f64).round()),
            ..JobRow::new(
                format!("ts-sync:{}", job.id),
                RowKind::Transfer,
                title,
                &job.name,
            )
        }
        .automatic(job.automatic),
    )
}

/// A backup: a person's from Settings, or the schedule's, which pauses for
/// a person's work and is listed as waiting meanwhile. Settings is a
/// dialog, not a view, so the row has no link.
fn backup_row(job: &BackupJob) -> JobRow {
    let scheduled = job.trigger == crate::backup::Trigger::Schedule;
    JobRow {
        detail: job.description(),
        status: if job.waiting_for.is_some() {
            RowStatus::Waiting
        } else {
            RowStatus::Running
        },
        waiting_for: job.waiting_for.clone(),
        percent: job.percent(),
        stoppable: !job.stopping,
        control: Some(RowControl::Backup),
        hint: Some(if job.location == Location::Downloads {
            "For download".into()
        } else {
            job.destination.clone()
        }),
        ..JobRow::new(
            format!("backup:{}", job.started_at),
            RowKind::Backup,
            if scheduled {
                "Scheduled backup"
            } else {
                "Backing up"
            },
            if job.scope == crate::backup::BackupScope::Full {
                "Settings and catalogs"
            } else {
                "Settings"
            },
        )
    }
    .automatic(scheduled)
}

/// A source-frame search for an artifact, in the stack line. Stop ends it
/// before its next frame.
fn artifact_search_row(
    search: &ArtifactSearchJob,
    position: Option<usize>,
    names: &HashMap<&str, &str>,
) -> JobRow {
    let queued = search.state == ArtifactSearchState::Queued;
    JobRow {
        detail: if queued {
            String::new()
        } else {
            search.phase.clone()
        },
        status: if queued {
            RowStatus::Queued
        } else {
            RowStatus::Running
        },
        percent: if queued {
            None
        } else {
            percent(search.completed_work_units, search.total_work_units)
        },
        queue: Some(RowQueue::Stack),
        position,
        control: Some(RowControl::Stack {
            job_id: search.search_id.clone(),
        }),
        stoppable: true,
        link: Some(RowLink::view(LinkView::Stacks, &search.database_id)),
        ..JobRow::new(
            format!("artifact:{}", search.search_id),
            RowKind::ArtifactSearch,
            "Searching source frames for an artifact",
            names
                .get(search.database_id.as_str())
                .copied()
                .unwrap_or(&search.database_id),
        )
    }
}

/// A preview a person waits for. It runs at once and Stop ends it.
fn preview_row(run: &PreviewRun, names: &HashMap<&str, &str>) -> JobRow {
    JobRow {
        control: Some(RowControl::Preview {
            preview: run.kind,
            key: run.key.clone(),
        }),
        stoppable: true,
        started_at: Some(run.started_at),
        ..JobRow::new(
            format!("preview:{:?}:{}", run.kind, run.key),
            RowKind::Preview,
            run.label.clone(),
            names.get(run.db_id.as_str()).copied().unwrap_or(&run.db_id),
        )
    }
}

/// A Seiza catalog installation a person started in Settings › Catalogs.
/// Settings is a dialog, not a view, so the row has no link.
fn catalog_install_row(progress: &CatalogInstallProgress) -> JobRow {
    let percent = match progress.bytes_total {
        Some(total) if total > 0 => {
            Some((100.0 * progress.bytes_completed.unwrap_or(0) as f64 / total as f64).min(100.0))
        }
        _ => (progress.files_total > 0).then(|| {
            (100.0 * progress.files_completed as f64 / progress.files_total as f64).min(100.0)
        }),
    };
    JobRow {
        detail: progress.message.clone(),
        percent,
        hint: progress.output_dir.clone(),
        control: Some(RowControl::CatalogInstall),
        stoppable: true,
        ..JobRow::new(
            format!("catalog:{}", progress.started_at.unwrap_or(0)),
            RowKind::Catalog,
            if progress.preset == Some(CatalogInstallPreset::GaiaPhotometry) {
                "Installing Gaia colors"
            } else {
                "Installing catalogs"
            },
            "Seiza catalogs",
        )
    }
}

/// A person's work first, then automatic work. Within each: work that needs
/// a person, running work, each line's queued rows in line order, then
/// waiting work. One line can hold rows of both tiers; a queued row's
/// position is its place in the whole line.
fn compare(left: &JobRow, right: &JobRow) -> std::cmp::Ordering {
    let tier = left.background.cmp(&right.background);
    if tier.is_ne() {
        return tier;
    }
    let status = left.status.order().cmp(&right.status.order());
    if status.is_ne() || left.status != RowStatus::Queued {
        return status;
    }
    // A line's order is its positions. A queued transfer has no line and
    // keeps the order it came in.
    let place = |row: &JobRow| {
        row.queue.map_or(2, RowQueue::order) * 1_000_000
            + row.position.map_or(999_999, |p| p as u64)
    };
    place(left).cmp(&place(right))
}

/// Every job's row, in the order the header lists them. An automatic
/// refresh still settling is not work yet; it is listed once it is due.
pub fn rows(snapshot: &Snapshot, project_of: ProjectOf) -> Vec<JobRow> {
    let mut rows = Vec::new();
    for db in &snapshot.databases {
        if let Some(refresh) = &db.refresh {
            rows.push(refresh_row(db, refresh));
        }
        rows.extend(quality_rows(db, project_of));
        if db.flats.active {
            rows.push(flats_row(db, &db.flats));
        }
        if db.sites.active {
            rows.push(sites_row(db, &db.sites));
        }
        if db.duplicates.state == "running" {
            rows.push(duplicates_row(db, &db.duplicates));
        }
        if db.rejects.state == "running" {
            rows.push(rejects_row(db, &db.rejects));
        }
        if db.grading.running && db.grading.to_measure > 0 {
            rows.push(grading_row(db, snapshot.interactive.as_deref()));
        }
        if db.health.running && db.health.total > 0 {
            rows.push(health_row(db));
        }
        if let Some(reads) = db.header_reads {
            rows.push(headers_row(db, reads));
        }
        if db.flat_use {
            rows.push(flat_use_row(db));
        }
        if let Some(row) = upkeep_row(db) {
            rows.push(row);
        }
        if let Some(import) = &db.import {
            rows.push(import_row(db, import));
        }
        if let Some(export) = &db.export {
            rows.push(export_row(db, export));
        }
    }
    // Running builds first, then the line in order.
    let mut stacks: Vec<&StackActivityEntry> = snapshot.stacks.active.iter().collect();
    stacks.sort_by_key(|entry| {
        (
            entry.state == StackJobState::Queued,
            entry.queue_position.unwrap_or(usize::MAX),
        )
    });
    rows.extend(stacks.into_iter().map(stack_row));
    rows.extend(
        snapshot
            .wbpp
            .running
            .iter()
            .map(|run| wbpp_running_row(run, project_of)),
    );
    rows.extend(
        snapshot
            .wbpp
            .queued
            .iter()
            .enumerate()
            .map(|(index, run)| wbpp_queued_row(run, index, project_of)),
    );
    rows.extend(snapshot.transfers.iter().filter_map(transfer_row));
    rows.extend(
        snapshot
            .stacks
            .scheduled
            .iter()
            .filter(|refresh| refresh.waiting)
            .map(waiting_refresh_row),
    );
    rows.extend(
        snapshot
            .stacks
            .waiting_reapplies
            .iter()
            .map(waiting_reapply_row),
    );
    if let Some(backup) = &snapshot.backup {
        rows.push(backup_row(backup));
    }
    if let Some(install) = snapshot.catalog_install.as_ref().filter(|p| p.running) {
        rows.push(catalog_install_row(install));
    }
    rows.extend(snapshot.ts_sync.iter().filter_map(ts_sync_row));
    let names: HashMap<&str, &str> = snapshot
        .databases
        .iter()
        .map(|db| (db.id.as_str(), db.name.as_str()))
        .collect();
    rows.extend(snapshot.previews.iter().map(|run| preview_row(run, &names)));
    rows.extend(snapshot.planning_pushes.iter().map(|peer| {
        // A site answered: the plans it was not sent go to it now.
        JobRow {
            background: true,
            automatic: true,
            ..JobRow::new(
                format!("planning-push:{peer}"),
                RowKind::PlanningPush,
                "Sending planning",
                peer,
            )
        }
    }));
    rows.extend(
        snapshot
            .artifact_searches
            .iter()
            .map(|(search, position)| artifact_search_row(search, *position, &names)),
    );
    // A stable sort: rows that compare equal keep the order above.
    rows.sort_by(compare);
    rows
}

/// Whether work that changes the database's images, metrics or grades runs.
/// A duplicate check and a removal preview change nothing.
fn database_busy(db: &DatabaseState) -> bool {
    db.refresh.is_some()
        || db.scan.running
        || db.backfill.running
        || db.flats.active
        || db.sites.active
        || (db.duplicates.state == "running" && db.duplicates.action.as_deref() == Some("repair"))
        || (db.rejects.state == "running" && db.rejects.action.as_deref() != Some("preview"))
}

/// Each database's state, for the header's effects.
pub fn databases(snapshot: &Snapshot, project_of: ProjectOf) -> Vec<DatabaseJobs> {
    snapshot
        .databases
        .iter()
        .map(|db| DatabaseJobs {
            id: db.id.clone(),
            name: db.name.clone(),
            busy: database_busy(db),
            scanning: db.scan.running || db.backfill.running,
            scan: ScanResult {
                errors: db.scan.errors,
                last_error: db.scan.last_error.clone(),
                target_id: db.scan.target_id,
                project_id: db
                    .scan
                    .target_id
                    .and_then(|target| project_of(&db.id, target)),
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The route.

/// Read every job's state from memory. Naming scheduled refreshes reads a
/// short query each, so call this off the async runtime.
fn snapshot(state: &Arc<AppState>, transfers: Vec<Value>, manages: bool) -> Snapshot {
    let databases = state
        .all_databases()
        .iter()
        .map(|ctx| DatabaseState {
            id: ctx.id.clone(),
            name: ctx.name.clone(),
            refresh: ctx.get_cache_refresh_progress(),
            scan: crate::server::spatial_scan::progress_snapshot(&ctx.spatial_metrics).0,
            backfill: crate::server::quality_backfill::snapshot(&ctx.quality_backfill),
            flats: crate::server::flat_measure::status(state, &ctx.database_path),
            sites: crate::server::frame_sites::status(state, &ctx.database_path),
            duplicates: crate::server::calibration_duplicates::status(state, &ctx.database_path),
            rejects: crate::server::reject_removal::status(state, &ctx.database_path),
            grading: crate::server::calibration_quality::progress(state, &ctx.database_path),
            health: crate::server::calibration_health::progress(state, &ctx.database_path),
            header_reads: crate::server::calibration_scope::header_reads(state, &ctx.database_path),
            flat_use: crate::server::flat_group_use::refreshing(state, &ctx.database_path),
            upkeep: crate::server::catalog_upkeep::status(state, &ctx.database_path),
            import: Some(crate::server::import_job::progress_snapshot(
                &ctx.import_job,
            ))
            .filter(|progress| progress.running && !progress.preview),
            export: Some(crate::server::export_job::progress_snapshot(
                &ctx.export_job,
            ))
            .filter(|progress| progress.running),
        })
        .collect();
    let backups_allowed = manages && crate::server::handlers::require_registry_path(state).is_ok();
    Snapshot {
        databases,
        stacks: crate::server::stack_preview::stack_activity(
            state,
            crate::server::stack_preview::RefreshPlanning::CachedOnly,
        ),
        wbpp: crate::server::wbpp_run::wbpp_activity(state),
        transfers,
        ts_sync: if manages {
            crate::server::ts_sync::listed_jobs(state)
                .into_iter()
                .map(TsSyncJob::from)
                .collect()
        } else {
            Vec::new()
        },
        backup: backups_allowed.then(|| state.backups.job()).flatten(),
        catalog_install: Some(state.catalog_install.status().progress),
        previews: state.previews.running(),
        artifact_searches: state.stack_previews.artifact_searches_under_way(),
        planning_pushes: if manages {
            crate::server::director::resends_under_way(state)
        } else {
            Vec::new()
        },
        interactive: state.interactive_job_name(),
    }
}

/// What the jobs route keeps between polls, for the life of the server
/// state.
#[derive(Default)]
pub struct JobsCache {
    /// Projects of targets read before, by database id and target id. A
    /// target's project never changes.
    target_projects: std::sync::Mutex<HashMap<(String, i32), i32>>,
    /// The transfer list of the last poll that read it in time.
    transfers: std::sync::Mutex<Vec<Value>>,
}

/// How long a poll waits for the Director's transfer list before it uses
/// the last list it read.
const TRANSFERS_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// The project of each target the rows link to. A target not cached yet is
/// read from the database's `target` table only when the shared connection
/// is free: the header's poll never waits for other work. A row whose
/// project is not known links to the database's images instead.
fn project_lookup(state: &AppState, snapshot: &Snapshot) -> HashMap<(String, i32), i32> {
    let mut wanted: Vec<(String, i32)> = Vec::new();
    for db in &snapshot.databases {
        for target in [db.scan.target_id, db.backfill.current_target_id]
            .into_iter()
            .flatten()
        {
            wanted.push((db.id.clone(), target));
        }
    }
    for run in &snapshot.wbpp.running {
        if let (None, Some(target)) = (run.project_id, run.target_id) {
            wanted.push((run.db_id.clone(), target));
        }
    }
    for run in &snapshot.wbpp.queued {
        if let (None, Some(target)) = (run.run.project_id, run.run.target_id) {
            wanted.push((run.db_id.clone(), target));
        }
    }
    let mut cached = state
        .jobs_cache
        .target_projects
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut projects = HashMap::new();
    for (db_id, target) in wanted {
        let key = (db_id.clone(), target);
        if projects.contains_key(&key) {
            continue;
        }
        if let Some(project) = cached.get(&key) {
            projects.insert(key, *project);
            continue;
        }
        let Some(ctx) = state.get_database(&db_id) else {
            continue;
        };
        let conn = ctx.db();
        let Ok(conn) = conn.try_lock() else {
            continue;
        };
        let project = conn
            .query_row(
                "SELECT projectid FROM target WHERE id = ?1",
                [target],
                |row| row.get::<_, i32>(0),
            )
            .ok();
        if let Some(project) = project {
            cached.insert(key.clone(), project);
            projects.insert(key, project);
        }
    }
    projects
}

/// Build the response from the server's state. `manages` is whether the
/// request may see backups and Target Scheduler syncs: an editor's request
/// on a server that allows database management.
fn build(state: &Arc<AppState>, transfers: Vec<Value>, manages: bool) -> JobsResponse {
    let snapshot = snapshot(state, transfers, manages);
    let projects = project_lookup(state, &snapshot);
    let project_of = |db: &str, target: i32| projects.get(&(db.to_string(), target)).copied();
    JobsResponse {
        jobs: rows(&snapshot, &project_of),
        databases: databases(&snapshot, &project_of),
    }
}

/// `GET /api/jobs` — every running, queued and waiting job on the server,
/// as the header lists them, and each database's state for the header's
/// effects. Backups and Target Scheduler syncs are listed only for an
/// editor on a server that allows database management, as their own
/// status routes are.
pub async fn get_jobs(
    State(state): State<Arc<AppState>>,
    access: Option<Extension<RequestAccess>>,
) -> Json<ApiResponse<JobsResponse>> {
    let editor = access
        .as_ref()
        .is_none_or(|Extension(access)| access.role == AccessRole::ReadWrite);
    let manages = editor && state.database_management_allowed();
    let transfers = match tokio::time::timeout(
        TRANSFERS_WAIT,
        crate::server::director::transfer_jobs(&state),
    )
    .await
    {
        Ok(transfers) => {
            *state
                .jobs_cache
                .transfers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = transfers.clone();
            transfers
        }
        // The Director's store is busy: the transfer rows of the last
        // poll stand in.
        Err(_) => state
            .jobs_cache
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    };
    let response = tokio::task::spawn_blocking(move || build(&state, transfers, manages))
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("Could not list the jobs: {error}");
            JobsResponse {
                jobs: Vec::new(),
                databases: Vec::new(),
            }
        });
    Json(ApiResponse::success(response))
}

#[cfg(test)]
mod tests;
