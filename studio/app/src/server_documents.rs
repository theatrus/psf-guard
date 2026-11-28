//! A picture's document on a PSF Guard server: opening one, and saving it
//! back as it is edited.
//!
//! A picture opened from a server target is linked to a document there. The
//! link sits beside the picture's document, in `<name>.server.json`: the
//! server, database, project and target, the document's id and the revision
//! this computer last saved or opened, and a hash of the text it sent.
//!
//! Studio saves the document on this computer as it does without a server.
//! A few seconds after edits stop, it also saves a new revision to the
//! server, with the file an external editor saved at the Finish junction.
//! When the server cannot be reached, it keeps saving here and tries again
//! later; it sends only the newest text. When another computer saved first,
//! Studio keeps this computer's edits and offers **Save as a copy** or
//! **Open the newer version**.
//!
//! The server keeps none of what Studio computes from the document: the
//! tile store, stacks and tool results are made again on each computer.

use super::servers::{
    kept_token, stacks_changed, Client, DocumentRecord, Failure, FileUse, SaveFailure,
    ServerRecord, StacksChanged,
};
use super::Studio;
use crate::studio_model::{Document, JunctionState, StageKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long edits must stop before a save goes to the server.
const SETTLE: Duration = Duration::from_secs(5);
/// How long Studio waits for a save to reach the server before it quits.
const QUIT_WAIT: Duration = Duration::from_secs(15);

/// Set when the person quits or closes the window. Studio's loop saves the
/// document here and sends changes not on the server yet, then quits.
pub static QUIT_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asks Studio to quit once its document is saved, here and on its server.
/// Studio quits after `QUIT_WAIT` and a second whatever happens.
pub fn quit_after_saving(cx: &mut gpui::App) {
    QUIT_ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(QUIT_WAIT + Duration::from_secs(1))
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}
/// How long to wait before trying an unreachable server again.
const RETRY: Duration = Duration::from_secs(30);
/// How often a save also sends a new preview for the document list. The
/// save made on quitting always sends one.
const PREVIEW_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long a save waits for the GPU to draw its preview before it goes
/// without one.
const PREVIEW_WAIT: Duration = Duration::from_secs(5);

/// A picture's link to its document on a server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerLink {
    pub server: String,
    pub database: String,
    pub project: i32,
    pub target: i32,
    /// The document's id; `None` until the first save creates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// The revision this computer last saved or opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u32>,
    /// A hash of the document text that revision holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<String>,
    /// Set when the revision was just opened: the text Studio reads it as
    /// is taken as sent, so opening alone saves nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
}

/// The link beside a document: `Iris.json` has `Iris.server.json`.
pub fn link_path(document: &Path) -> PathBuf {
    document.with_extension("server.json")
}

impl ServerLink {
    pub fn load(document: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(link_path(document)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, document: &Path) -> Result<(), String> {
        let path = link_path(document);
        let temporary = path.with_extension("saving");
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|error| error.to_string())
    }
}

/// A hash of a document's text, to tell whether it was sent.
pub fn text_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Where the server copy of a picture's document stands.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveState {
    /// Every change is on the server.
    Saved,
    Saving,
    /// Saved on this computer only, with why.
    LocalOnly(String),
    /// Another computer saved the revision this one is after.
    Conflict(u32),
}

/// What a save on its own thread came to.
type Outcome = Result<(DocumentRecord, String), SaveFailure>;
/// What sending exports to the project came to.
type ExportOutcome = Result<(), Failure>;

#[derive(Default)]
pub(super) struct ServerSave {
    /// The link, and the document file it belongs to.
    link: Option<(PathBuf, ServerLink)>,
    pub state: Option<SaveState>,
    /// When the document text last changed, and its hash then.
    changed: Option<(Instant, String)>,
    running: Option<Arc<Mutex<Option<Outcome>>>>,
    /// Exports being sent to the project.
    exporting: Option<Arc<Mutex<Option<ExportOutcome>>>>,
    retry_after: Option<Instant>,
    /// Exports to save to the project once the document is saved, each with
    /// its preview when one was written.
    exports: Vec<(PathBuf, Option<PathBuf>)>,
    /// Whether an export is also saved to the project.
    pub export_to_project: bool,
    /// When the person asked to quit while changes were unsent.
    quit_asked_at: Option<Instant>,
    /// When a save last sent a preview, or gave up waiting for one.
    preview_at: Option<Instant>,
    /// The preview asked of the render thread for the next save: its id,
    /// when it was asked, and the picture's peak once the request is made.
    preview_asked: Option<PreviewAsked>,
    /// Counts previews asked for, to tell their requests apart.
    previews: u64,
}

/// A preview asked of the render thread: its id, when it was asked, and
/// the picture's peak once the request is made.
type PreviewAsked = (u64, Instant, Arc<Mutex<Option<f32>>>);

/// A preview for a save: the render thread's image and the picture's peak.
pub(super) struct SavePreview {
    image: super::PreviewImage,
    peak: f32,
}

/// What a save does about its preview.
enum PreviewStep {
    /// Send this one.
    Ready(SavePreview),
    /// The GPU is drawing it: the save waits.
    Waiting,
    /// The save goes without one.
    Without,
}

impl ServerSave {
    /// Whether the open picture is linked to a document on a server.
    pub fn linked(&self) -> bool {
        self.link.is_some()
    }

    pub fn conflict(&self) -> bool {
        matches!(self.state, Some(SaveState::Conflict(_)))
    }

    pub fn new() -> Self {
        Self {
            export_to_project: true,
            ..Self::default()
        }
    }
}

/// The edited file at the Finish junction, by hash, when there is one.
fn edited_file(document: &Document) -> Option<(PathBuf, String)> {
    match &document.stage(StageKind::Finish).junction.state {
        JunctionState::Edited { path, edited, .. } => Some((path.clone(), edited.sha256.clone())),
        _ => None,
    }
}

/// The stacks a picture was made from, from the sources `psf-guard studio`
/// wrote beside it.
fn stacks_of(document: &Path) -> Value {
    let Some(stem) = document.file_stem() else {
        return json!([]);
    };
    let sources = document
        .with_file_name(format!("{}.sources", stem.to_string_lossy()))
        .join("sources.json");
    let Some(list) = std::fs::read_to_string(sources)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
    else {
        return json!([]);
    };
    Value::Array(
        list.iter()
            .map(|stack| {
                json!({
                    "filter_name": stack["filter_name"],
                    "job_id": stack["job_id"],
                    "group_index": stack["group_index"],
                    "artifact_revision": stack["artifact_revision"],
                    "reference_image_id": stack["reference_image_id"],
                    "width": stack["sky_orientation"]["output_width"],
                    "height": stack["sky_orientation"]["output_height"],
                })
            })
            .collect(),
    )
}

/// Saves `text` to the server: the edited file first, then a new document
/// or a revision of the linked one.
fn send(
    client: &Client,
    link: &ServerLink,
    name: &str,
    text: &str,
    document_path: &Path,
    preview: Option<SavePreview>,
) -> Outcome {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| SaveFailure::Failed(Failure::Failed(error.to_string())))?;
    let preview = match preview {
        Some(preview) => {
            send_preview(client, &link.database, preview).map_err(SaveFailure::Failed)?
        }
        None => None,
    };
    let mut files = Vec::new();
    if let Ok(parsed) = Document::from_json(text)
        && let Some((path, sha)) = edited_file(&parsed)
        && path.is_file()
    {
        client
            .upload_file(&link.database, &sha, &path)
            .map_err(SaveFailure::Failed)?;
        files.push(FileUse {
            role: "edited".to_string(),
            sha256: sha,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
        });
    }
    let record = match (&link.document_id, link.revision) {
        (Some(id), Some(revision)) => client.save_document(
            &link.database,
            id,
            json!({
                "base_revision": revision,
                "document": document,
                "files": files,
                "preview": preview,
                "stacks": stacks_of(document_path),
                "saved_from": super::servers::this_computer(),
            }),
        )?,
        _ => client
            .create_document(
                &link.database,
                json!({
                    "name": name,
                    "project_id": link.project,
                    "target_id": link.target,
                    "stacks": stacks_of(document_path),
                    "document": document,
                    "files": files,
                    "preview": preview,
                    "saved_from": super::servers::this_computer(),
                }),
            )
            .map_err(SaveFailure::Failed)?,
    };
    Ok((record, text_hash(text)))
}

/// Writes a save's preview as a small PNG and uploads it. Returns its hash,
/// or `None` when the PNG could not be written: the save then goes without
/// a preview.
fn send_preview(
    client: &Client,
    database: &str,
    preview: SavePreview,
) -> Result<Option<String>, Failure> {
    let path = std::env::temp_dir()
        .join("psf-studio-previews")
        .join(format!("{}.png", uuid::Uuid::new_v4().simple()));
    let image = &preview.image;
    if let Err(reason) = super::export::write_preview(
        &path,
        image.width as usize,
        image.height as usize,
        &image.rgb,
        preview.peak,
    ) {
        eprintln!("the preview was not written: {reason}");
        return Ok(None);
    }
    let sent = crate::studio_model::FileStamp::of(&path)
        .map_err(|error| Failure::Failed(error.to_string()))
        .and_then(|stamp| {
            client.upload_file(database, &stamp.sha256, &path)?;
            Ok(stamp.sha256)
        });
    let _ = std::fs::remove_file(&path);
    sent.map(Some)
}

/// Uploads an export, and its preview, and records it against the
/// document's revision.
fn send_export(
    client: &Client,
    link: &ServerLink,
    path: &Path,
    preview: Option<&Path>,
) -> Result<(), Failure> {
    let (Some(id), Some(revision)) = (&link.document_id, link.revision) else {
        return Err(Failure::Failed(
            "The document is not on the server yet".to_string(),
        ));
    };
    let stamp = crate::studio_model::FileStamp::of(path)
        .map_err(|error| Failure::Failed(error.to_string()))?;
    client.upload_file(&link.database, &stamp.sha256, path)?;
    let preview = match preview.filter(|preview| preview.is_file()) {
        Some(preview) => {
            let sha = crate::studio_model::FileStamp::of(preview)
                .map_err(|error| Failure::Failed(error.to_string()))?
                .sha256;
            client.upload_file(&link.database, &sha, preview)?;
            let _ = std::fs::remove_file(preview);
            Some(sha)
        }
        None => None,
    };
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().to_string());
    let format = path.extension().map_or_else(String::new, |extension| {
        extension.to_string_lossy().to_string()
    });
    client.add_export(
        &link.database,
        id,
        json!({
            "sha256": stamp.sha256,
            "revision": revision,
            "name": name,
            "format": format,
            "preview": preview,
        }),
    )
}

/// Prepares a document from a server for this computer: the edited file it
/// uses downloaded beside it, and its junction pointed there. A junction
/// whose file stayed on another computer reads Studio's own result.
pub fn localize(
    client: &Client,
    database: &str,
    uses: &[FileUse],
    text: &str,
    document_path: &Path,
) -> Result<String, Failure> {
    let mut document =
        Document::from_json(text).map_err(|error| Failure::Failed(error.to_string()))?;
    let junction = document.junction_mut(StageKind::Finish);
    match &mut junction.state {
        JunctionState::Edited { path, edited, .. } => {
            match uses.iter().find(|file| file.role == "edited") {
                Some(file) if file.sha256 == edited.sha256 => {
                    let local = document_path.with_extension("edit.tif");
                    client.download_file(database, &file.sha256, &local)?;
                    *edited = crate::studio_model::FileStamp::of(&local)
                        .map_err(|error| Failure::Failed(error.to_string()))?;
                    *path = local;
                }
                _ => junction.use_computed(),
            }
        }
        JunctionState::Exported { .. } => junction.use_computed(),
        JunctionState::Computed => {}
    }
    Ok(document.to_json())
}

impl Studio {
    /// The link of the picture open now, read again when another picture
    /// opened.
    fn current_link(&mut self) -> Option<ServerLink> {
        let path = self.document_path()?.to_path_buf();
        let save = &mut self.server_save;
        if save.link.as_ref().map(|(known, _)| known) != Some(&path) {
            save.link = ServerLink::load(&path).map(|link| (path.clone(), link));
            save.state = None;
            save.changed = None;
            save.retry_after = None;
        }
        save.link.as_ref().map(|(_, link)| link.clone())
    }

    fn server_record(&self, address: &str) -> Option<ServerRecord> {
        self.servers.record(address)
    }

    /// Saves the open document to its server once edits have stopped, and
    /// takes in a save that finished.
    pub(super) fn follow_server_save(&mut self) {
        self.follow_running_save();
        self.send_when_settled();
    }

    /// Whether the open picture has changes or exports not on its server,
    /// which a save can still send.
    fn has_unsent(&self) -> bool {
        let save = &self.server_save;
        let Some((_, link)) = &save.link else {
            return false;
        };
        if matches!(
            save.state,
            Some(SaveState::Conflict(_)) | Some(SaveState::LocalOnly(_))
        ) {
            return save.running.is_some();
        }
        save.running.is_some()
            || save.exporting.is_some()
            || !save.exports.is_empty()
            || link.opened
            || link.sent.as_deref() != Some(text_hash(&self.document.to_json()).as_str())
    }

    /// Quits once a save asked for by quitting has reached the server, or
    /// cannot, or has taken too long.
    pub(super) fn follow_quit(&mut self, cx: &mut gpui::App) {
        if !QUIT_ASKED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        // The last edit is saved here first, however soon the quit came.
        self.save_document();
        let asked = *self
            .server_save
            .quit_asked_at
            .get_or_insert_with(Instant::now);
        if !self.has_unsent() || asked.elapsed() >= QUIT_WAIT {
            cx.quit();
        }
    }

    /// Starts a save once edits have stopped, or at once when quitting.
    fn send_when_settled(&mut self) {
        let Some(link) = self.current_link() else {
            return;
        };
        let Some(document_path) = self.document_path().map(Path::to_path_buf) else {
            return;
        };
        let Some(record) = self.server_record(&link.server) else {
            self.server_save.state = Some(SaveState::LocalOnly(format!(
                "Studio is not connected to {}",
                link.server
            )));
            return;
        };
        if !record.saves_documents {
            self.server_save.state =
                Some(SaveState::LocalOnly("the account is read-only".to_string()));
            return;
        }
        if matches!(self.server_save.state, Some(SaveState::Conflict(_))) {
            return;
        }
        let text = self.document.to_json();
        let hash = text_hash(&text);
        if link.opened {
            let mut link = link.clone();
            link.opened = false;
            link.sent = Some(hash.clone());
            let _ = link.save(&document_path);
            self.server_save.link = Some((document_path, link));
            self.server_save.state = Some(SaveState::Saved);
            return;
        }
        let save = &mut self.server_save;
        if link.sent.as_deref() == Some(hash.as_str()) {
            save.changed = None;
            if save.running.is_none() {
                save.state = Some(SaveState::Saved);
            }
            self.follow_exports(&link);
            return;
        }
        // Wait until edits stop, unless an export waits on this save.
        match &save.changed {
            Some((_, known)) if *known == hash => {}
            _ => save.changed = Some((Instant::now(), hash.clone())),
        }
        // Changes wait here until edits stop.
        if !matches!(save.state, Some(SaveState::LocalOnly(_))) {
            save.state = Some(SaveState::Saving);
        }
        let quitting = QUIT_ASKED.load(std::sync::atomic::Ordering::Relaxed);
        let settled = save
            .changed
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= SETTLE)
            || !save.exports.is_empty()
            || quitting;
        let waiting = !quitting && save.retry_after.is_some_and(|at| Instant::now() < at);
        if save.running.is_some() || !settled || waiting {
            return;
        }
        let token = if record.signs_in {
            match kept_token(&record.address) {
                Some(token) => Some(token),
                None => {
                    save.state = Some(SaveState::LocalOnly("Studio is signed out".to_string()));
                    return;
                }
            }
        } else {
            None
        };
        let preview = match self.preview_for_save(quitting) {
            PreviewStep::Ready(preview) => Some(preview),
            PreviewStep::Waiting => return,
            PreviewStep::Without => None,
        };
        let client = Client::new(&record.address, token);
        let name = self.document.name.clone();
        let slot = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        std::thread::spawn(move || {
            let outcome = send(&client, &link, &name, &text, &document_path, preview);
            *filled.lock().expect("server save") = Some(outcome);
        });
        let save = &mut self.server_save;
        save.running = Some(slot);
        save.state = Some(SaveState::Saving);
    }

    /// The preview the next save sends: one is sent at most every
    /// `PREVIEW_EVERY`, and always on quitting. The GPU draws it from the
    /// canvas's tiles, so it shows the picture as the canvas does.
    fn preview_for_save(&mut self, quitting: bool) -> PreviewStep {
        let due = quitting
            || self
                .server_save
                .preview_at
                .is_none_or(|at| at.elapsed() >= PREVIEW_EVERY);
        if !due {
            return PreviewStep::Without;
        }
        if let Some((id, asked, peak)) = self.server_save.preview_asked.clone() {
            let drawn = self.canvas.read_mut(|shared| match &shared.preview_image {
                Some((known, _)) if *known == id => shared.preview_image.take(),
                _ => None,
            });
            let peak = *peak.lock().expect("preview peak");
            if let (Some((_, image)), Some(peak)) = (drawn, peak) {
                self.finish_preview();
                return PreviewStep::Ready(SavePreview { image, peak });
            }
            if asked.elapsed() < PREVIEW_WAIT {
                return PreviewStep::Waiting;
            }
            // The GPU did not draw it in time: this save, and the next ones
            // until the preview is due again, go without one.
            self.finish_preview();
            return PreviewStep::Without;
        }
        // The picture sampled for an output stretch must be the one the
        // canvas shows, as for an export.
        if self.store_path().is_none()
            || (self.picture_wanted.is_some()
                && self.picture_samples.as_ref().map(|(id, _)| *id) != self.picture_wanted)
        {
            return PreviewStep::Without;
        }
        self.ask_for_preview();
        PreviewStep::Waiting
    }

    /// Asks the render thread for a preview of the final picture. The scene
    /// is built on a thread of its own, as an export's is.
    fn ask_for_preview(&mut self) {
        let save = &mut self.server_save;
        save.previews += 1;
        let id = save.previews;
        let peak = Arc::new(Mutex::new(None));
        save.preview_asked = Some((id, Instant::now(), peak.clone()));
        let document = self.document.clone();
        let band_info = self.band_info.clone();
        let picture_samples = self.picture_samples.clone();
        let scene_version = self.scene_version;
        let resolved = self.scene_worker.resolved();
        let crop = self.crop_rect();
        let canvas = self.canvas.clone();
        std::thread::spawn(move || {
            let (mut final_scene, states) = super::panes::scene(
                &document,
                &band_info,
                &crate::studio_model::Place::Final.view(),
                scene_version,
                picture_samples
                    .as_ref()
                    .map(|(id, samples)| (*id, samples.as_slice())),
                &mut crate::output_stretch::ResolvedChains::from_resolved(&resolved),
                &[],
            );
            final_scene.stretches = Arc::new(band_info.stretches(&states).unwrap_or_default());
            let picture = final_scene.picture(&[]);
            *peak.lock().expect("preview peak") = Some(picture.peak());
            canvas.edit(|state| {
                state.preview = Some(Arc::new(super::PreviewRequest { id, picture, crop }));
            });
        });
    }

    /// Ends the preview under way: the render thread draws no more of it.
    fn finish_preview(&mut self) {
        self.server_save.preview_asked = None;
        self.server_save.preview_at = Some(Instant::now());
        self.canvas.edit(|state| state.preview = None);
    }

    fn follow_running_save(&mut self) {
        let Some(slot) = &self.server_save.running else {
            return;
        };
        let Some(outcome) = slot.lock().expect("server save").take() else {
            return;
        };
        self.server_save.running = None;
        let Some((path, mut link)) = self.server_save.link.clone() else {
            return;
        };
        match outcome {
            Ok((record, sent)) => {
                link.document_id = Some(record.id.clone());
                link.revision = record.current().map(|current| current.revision);
                link.sent = Some(sent);
                if let Err(reason) = link.save(&path) {
                    self.server_save.state = Some(SaveState::LocalOnly(format!(
                        "the link was not saved: {reason}"
                    )));
                }
                let target = (link.server.clone(), link.project, link.target);
                self.server_save.link = Some((path, link));
                self.server_save.retry_after = None;
                self.server_save.state = Some(SaveState::Saved);
                self.refresh_documents_of(&target);
            }
            Err(SaveFailure::Conflict(record)) => {
                let revision = record.current().map_or(0, |current| current.revision);
                self.server_save.state = Some(SaveState::Conflict(revision));
            }
            Err(SaveFailure::Failed(failure)) => {
                self.server_save.retry_after = Some(Instant::now() + RETRY);
                self.server_save.state = Some(SaveState::LocalOnly(match failure {
                    Failure::SignedOut => "Studio is signed out of the server".to_string(),
                    other => other.to_string(),
                }));
            }
        }
    }

    /// Sends exports waiting for the project, now that the document is on
    /// the server, and takes in sends that finished.
    fn follow_exports(&mut self, link: &ServerLink) {
        if let Some(slot) = &self.server_save.exporting {
            let Some(outcome) = slot.lock().expect("export save").take() else {
                return;
            };
            self.server_save.exporting = None;
            if let Err(failure) = outcome {
                self.tell_export(&format!(
                    "The export was not saved to the project: {failure}"
                ));
            } else {
                self.tell_export("The export is saved to the project.");
            }
        }
        if self.server_save.exports.is_empty() {
            return;
        }
        let Some(record) = self.server_record(&link.server) else {
            return;
        };
        let token = record
            .signs_in
            .then(|| kept_token(&record.address))
            .flatten();
        let client = Client::new(&record.address, token);
        let exports = std::mem::take(&mut self.server_save.exports);
        let link = link.clone();
        let slot = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        std::thread::spawn(move || {
            let outcome = exports.iter().try_for_each(|(path, preview)| {
                send_export(&client, &link, path, preview.as_deref())
            });
            *filled.lock().expect("export save") = Some(outcome);
        });
        self.server_save.exporting = Some(slot);
        self.tell_export("Saving the export to the project…");
    }

    /// An export finished: saves it to the project too, when the picture is
    /// on a server and the person asked for that.
    pub(super) fn export_finished(&mut self, path: &Path, preview: Option<PathBuf>) {
        if self.current_link().is_none() {
            return;
        }
        if self.server_save.export_to_project {
            self.server_save.exports.push((path.to_path_buf(), preview));
        } else if let Some(preview) = preview {
            let _ = std::fs::remove_file(preview);
        }
    }

    /// The server copy's state, as the top bar words it.
    pub(super) fn server_save_line(&self) -> Option<String> {
        let (_, link) = self.server_save.link.as_ref()?;
        let host = link
            .server
            .split("://")
            .nth(1)
            .unwrap_or(&link.server)
            .to_string();
        if QUIT_ASKED.load(std::sync::atomic::Ordering::Relaxed) {
            return Some(format!("Saving to {host} before quitting…"));
        }
        Some(match self.server_save.state.as_ref()? {
            SaveState::Saved => format!("Saved to {host}"),
            SaveState::Saving => format!("Saving to {host}…"),
            SaveState::LocalOnly(reason) => format!("Saved on this computer only: {reason}"),
            SaveState::Conflict(revision) => format!(
                "Revision {revision} was saved on {host} from another computer. Your edits are \
                 kept here."
            ),
        })
    }

    /// Puts the open picture on a server target as a new document: it is
    /// linked to the target, and the next save creates the document there.
    pub(super) fn save_picture_to(
        &mut self,
        target: &super::open::TargetLink,
    ) -> Result<(), String> {
        let path = self
            .document_path()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Open a picture first".to_string())?;
        if self.server_save.link.is_some() {
            return Err("This picture is on a server already".to_string());
        }
        let link = ServerLink {
            server: target.server.clone(),
            database: target.database.clone(),
            project: target.project,
            target: target.target,
            ..ServerLink::default()
        };
        link.save(&path)?;
        self.server_save.link = Some((path, link));
        self.server_save.state = None;
        self.server_save.changed = None;
        Ok(())
    }

    /// Saves this computer's edits as a new document on the server.
    pub(super) fn save_as_copy(&mut self) {
        let Some((path, mut link)) = self.server_save.link.clone() else {
            return;
        };
        link.document_id = None;
        link.revision = None;
        link.sent = None;
        self.document.name = format!("{} (copy)", self.document.name);
        let _ = link.save(&path);
        self.server_save.link = Some((path, link));
        self.server_save.state = None;
        self.server_save.changed = None;
        self.server_save.retry_after = None;
    }

    /// Discards this computer's edits and opens the server's newest
    /// revision.
    pub(super) fn open_newer_version(&mut self) {
        let Some((_, link)) = self.server_save.link.clone() else {
            return;
        };
        let Some(id) = link.document_id.clone() else {
            return;
        };
        self.server_save.state = None;
        self.open_server_document(
            super::open::TargetLink {
                server: link.server,
                database: link.database,
                project: link.project,
                target: link.target,
                ticket: None,
                document: None,
            },
            Which::Document(id),
            true,
        );
    }
}

/// Which of a target's documents to open.
#[derive(Clone, Debug, PartialEq)]
pub enum Which {
    /// The most recently saved, or a new one when there is none.
    Latest,
    New,
    Document(String),
    /// An earlier revision of a document.
    Revision(String, u32),
}

/// Fetches a document for this computer: its newest revision, or `at`,
/// written to `document_path`, with its edited file beside it, and the
/// link. The copy here is kept when it holds edits not yet sent, unless
/// `discard`.
///
/// The link names the newest revision even when an earlier one is read, so
/// edits to an earlier revision are saved as a new revision after the
/// newest.
fn fetch(
    client: &Client,
    target: &super::open::TargetLink,
    record: &DocumentRecord,
    document_path: &Path,
    discard: bool,
    at: Option<u32>,
) -> Result<(), Failure> {
    let id = record.id.as_str();
    let revision = record
        .current()
        .map(|current| current.revision)
        .ok_or_else(|| Failure::Failed("The document has no revision".to_string()))?;
    let read = at.unwrap_or(revision);
    let entry = record
        .revisions
        .iter()
        .find(|entry| entry.revision == read)
        .ok_or_else(|| Failure::Failed(format!("The server no longer keeps revision {read}")))?;
    let local = ServerLink::load(document_path);
    let unsent = local.as_ref().is_some_and(|link| {
        std::fs::read_to_string(document_path)
            .is_ok_and(|text| link.sent.as_deref() != Some(text_hash(&text).as_str()))
    });
    let keep_local = !discard
        && unsent
        && local
            .as_ref()
            .is_some_and(|link| link.document_id.as_deref() == Some(id));
    if !keep_local {
        let text = client.revision_text(&target.database, id, read)?;
        let text = localize(client, &target.database, &entry.files, &text, document_path)?;
        let temporary = document_path.with_extension("json.saving");
        std::fs::write(&temporary, &text)
            .and_then(|()| std::fs::rename(&temporary, document_path))
            .map_err(|error| Failure::Failed(error.to_string()))?;
    }
    ServerLink {
        server: target.server.clone(),
        database: target.database.clone(),
        project: target.project,
        target: target.target,
        document_id: Some(id.to_string()),
        // Edits kept here were made from the revision they were made from.
        revision: if keep_local {
            local.and_then(|link| link.revision)
        } else {
            Some(revision)
        },
        sent: None,
        opened: !keep_local,
    }
    .save(document_path)
    .map_err(Failure::Failed)
}

/// What fetching a document came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Fetched {
    /// Ready to prepare: the picture's name, and a note for the person.
    Ready { name: String, note: Option<String> },
    /// The target's stacks were registered anew since the document was
    /// saved: the person chooses whether to open it with them.
    Reframed(Which),
    /// The target has no channel stacks, which Studio opens a target from:
    /// the person is asked to build them in PSF Guard.
    NoStacks,
}

/// A document of a target being made ready to open.
pub(super) type Fetching = Arc<Mutex<Option<Result<Fetched, Failure>>>>;

/// What a person reads when a document's stacks were registered anew.
pub const REFRAMED: &str = "The target's stacks were registered to another frame or at another \
     size since this document was saved. Masks, heals and the crop are in the old stacks' \
     pixels, so they may fall in the wrong place.";

impl Studio {
    /// Opens one of a target's documents on a server: the document is
    /// fetched, then the picture is prepared as a link prepares it.
    pub(super) fn open_server_document(
        &mut self,
        target: super::open::TargetLink,
        which: Which,
        discard: bool,
    ) {
        self.open_server_document_with(target, which, discard, false);
    }

    /// Opens a document; `new_stacks` opens it even when its stacks were
    /// registered anew.
    pub(super) fn open_server_document_with(
        &mut self,
        target: super::open::TargetLink,
        which: Which,
        discard: bool,
        new_stacks: bool,
    ) {
        let Some(record) = self.server_record(&target.server) else {
            self.say("Not opened: Studio is not connected to that server");
            return;
        };
        let token = if record.signs_in {
            match kept_token(&record.address) {
                Some(token) => Some(token),
                None => {
                    self.say("Not opened: Studio is signed out of that server. Connect again.");
                    return;
                }
            }
        } else {
            None
        };
        let client = Client::new(&record.address, token.clone());
        let base = super::open::target_picture_name(&target);
        let folder = super::open::pictures_folder();
        let slot: Fetching = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        let fetching_target = target.clone();
        std::thread::spawn(move || {
            let outcome = (|| {
                std::fs::create_dir_all(&folder)
                    .map_err(|error| Failure::Failed(error.to_string()))?;
                // A target opens from its channel stacks; without them there
                // is nothing to prepare.
                let now = client
                    .stacked_targets(&target.database, target.project)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|stacked| stacked.target_id == target.target)
                    .map(|stacked| stacked.source_candidates)
                    .unwrap_or_default();
                if now.is_empty() {
                    return Ok(Fetched::NoStacks);
                }
                let asked = which.clone();
                let mut at = None;
                let id = match which {
                    Which::Document(id) => Some(id),
                    Which::Revision(id, revision) => {
                        at = Some(revision);
                        Some(id)
                    }
                    Which::New => None,
                    Which::Latest => client
                        .documents(&target.database, target.project, target.target)?
                        .first()
                        .map(|summary| summary.id.clone()),
                };
                let Some(id) = id else {
                    // A new document: its link has no id until the first
                    // save creates it.
                    let random = uuid::Uuid::new_v4().simple().to_string();
                    let name = format!("{base}-new-{}", &random[..8]);
                    ServerLink {
                        server: target.server.clone(),
                        database: target.database.clone(),
                        project: target.project,
                        target: target.target,
                        ..ServerLink::default()
                    }
                    .save(&folder.join(format!("{name}.json")))
                    .map_err(Failure::Failed)?;
                    return Ok(Fetched::Ready { name, note: None });
                };
                let record = client.document_record(&target.database, &id)?;
                let latest = record.current().map_or(0, |current| current.revision);
                let at = at.filter(|revision| *revision != latest);
                let stacks_note = match stacks_changed(&record.stacks, &now) {
                    StacksChanged::No => None,
                    StacksChanged::Rebuilt => Some(
                        "The target's stacks were built again since this document was saved. \
                         Studio opens it with the new stacks."
                            .to_string(),
                    ),
                    StacksChanged::Reframed if !new_stacks => {
                        return Ok(Fetched::Reframed(asked));
                    }
                    StacksChanged::Reframed => None,
                };
                let revision_note = at.map(|revision| {
                    format!(
                        "Revision {revision} is open. Studio saves your edits to it as a new \
                         revision, after revision {latest}."
                    )
                });
                let note = match (revision_note, stacks_note) {
                    (Some(first), Some(second)) => Some(format!("{first} {second}")),
                    (first, second) => first.or(second),
                };
                // An earlier revision is a picture of its own here, so it
                // leaves edits to the newest one alone.
                let short = &id.replace('-', "")[..8.min(id.len())];
                let name = match at {
                    Some(revision) => format!("{base}-{short}-r{revision}"),
                    None => format!("{base}-{short}"),
                };
                fetch(
                    &client,
                    &target,
                    &record,
                    &folder.join(format!("{name}.json")),
                    discard,
                    at,
                )?;
                Ok(Fetched::Ready { name, note })
            })();
            *filled.lock().expect("fetching") = Some(outcome);
        });
        self.servers.fetching = Some((fetching_target, token, slot, discard));
        self.say("Fetching the document from the server…");
    }

    /// Prepares a fetched document's picture once it is here.
    pub(super) fn follow_fetching(&mut self) {
        let Some((_, _, slot, _)) = &self.servers.fetching else {
            return;
        };
        let Some(outcome) = slot.lock().expect("fetching").take() else {
            return;
        };
        let Some((target, token, _, discard)) = self.servers.fetching.take() else {
            return;
        };
        match outcome {
            Ok(Fetched::Ready { name, note }) => {
                self.open_target_named(&target, token, &name);
                if let Some(note) = note {
                    self.say(&note);
                }
            }
            Ok(Fetched::Reframed(which)) => {
                self.servers.reframed = Some((target, which, discard));
                self.servers.open = true;
            }
            Ok(Fetched::NoStacks) => {
                self.say("Not opened: the target has no channel stacks yet");
                self.servers.no_stacks = Some(target);
                self.servers.open = true;
            }
            Err(failure) => self.say(&format!("The document was not fetched: {failure}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_sits_beside_its_document_and_reads_back() {
        let folder = tempfile::tempdir().unwrap();
        let document = folder.path().join("Iris.json");
        assert_eq!(link_path(&document), folder.path().join("Iris.server.json"));
        assert_eq!(ServerLink::load(&document), None);
        let link = ServerLink {
            server: "http://localhost:3000".to_string(),
            database: "main".to_string(),
            project: 11,
            target: 17,
            document_id: Some("a".to_string()),
            revision: Some(3),
            sent: Some(text_hash("{}")),
            opened: false,
        };
        link.save(&document).unwrap();
        assert_eq!(ServerLink::load(&document), Some(link));
    }

    #[test]
    fn the_stacks_a_picture_was_made_from_come_from_its_sources() {
        let folder = tempfile::tempdir().unwrap();
        let document = folder.path().join("Iris.json");
        std::fs::create_dir_all(folder.path().join("Iris.sources")).unwrap();
        std::fs::write(
            folder.path().join("Iris.sources/sources.json"),
            r#"[{"filter_name":"Ha","job_id":"j","group_index":0,"artifact_revision":"r","role":"red"}]"#,
        )
        .unwrap();
        assert_eq!(
            stacks_of(&document),
            json!([{"filter_name":"Ha","job_id":"j","group_index":0,"artifact_revision":"r",
                "reference_image_id":null,"width":null,"height":null}])
        );
        assert_eq!(stacks_of(&folder.path().join("Other.json")), json!([]));
    }

    #[test]
    fn an_exported_junction_reads_studios_result_on_another_computer() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("out.tif");
        std::fs::write(&file, b"pixels").unwrap();
        let mut document = Document::new("Iris");
        document
            .junction_mut(StageKind::Finish)
            .exported(&file, "upstream")
            .unwrap();
        // No request is made for an exported junction.
        let client = Client::new("http://127.0.0.1:9", None);
        let text = localize(
            &client,
            "main",
            &[],
            &document.to_json(),
            &folder.path().join("Iris.json"),
        )
        .unwrap();
        let localized = Document::from_json(&text).unwrap();
        assert_eq!(
            localized.stage(StageKind::Finish).junction.state,
            JunctionState::Computed
        );
    }
}
