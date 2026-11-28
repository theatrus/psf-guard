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
    kept_token, Client, DocumentRecord, Failure, FileUse, SaveFailure, ServerRecord,
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
/// How long to wait before trying an unreachable server again.
const RETRY: Duration = Duration::from_secs(30);

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
    /// Exports to save to the project once the document is saved.
    exports: Vec<PathBuf>,
    /// Whether an export is also saved to the project.
    pub export_to_project: bool,
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
) -> Outcome {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| SaveFailure::Failed(Failure::Failed(error.to_string())))?;
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
            json!({ "base_revision": revision, "document": document, "files": files }),
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
                }),
            )
            .map_err(SaveFailure::Failed)?,
    };
    Ok((record, text_hash(text)))
}

/// Uploads an export and records it against the document's revision.
fn send_export(client: &Client, link: &ServerLink, path: &Path) -> Result<(), Failure> {
    let (Some(id), Some(revision)) = (&link.document_id, link.revision) else {
        return Err(Failure::Failed(
            "The document is not on the server yet".to_string(),
        ));
    };
    let stamp = crate::studio_model::FileStamp::of(path)
        .map_err(|error| Failure::Failed(error.to_string()))?;
    client.upload_file(&link.database, &stamp.sha256, path)?;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().to_string());
    let format = path.extension().map_or_else(String::new, |extension| {
        extension.to_string_lossy().to_string()
    });
    client.add_export(
        &link.database,
        id,
        json!({ "sha256": stamp.sha256, "revision": revision, "name": name, "format": format }),
    )
}

/// Prepares a document from a server for this computer: the edited file it
/// uses downloaded beside it, and its junction pointed there. A junction
/// whose file stayed on another computer reads Studio's own result.
pub fn localize(
    client: &Client,
    database: &str,
    record: &DocumentRecord,
    text: &str,
    document_path: &Path,
) -> Result<String, Failure> {
    let mut document =
        Document::from_json(text).map_err(|error| Failure::Failed(error.to_string()))?;
    let junction = document.junction_mut(StageKind::Finish);
    match &mut junction.state {
        JunctionState::Edited { path, edited, .. } => {
            let uses = record
                .current()
                .and_then(|current| current.files.iter().find(|file| file.role == "edited"));
            match uses {
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
        let settled = save
            .changed
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= SETTLE)
            || !save.exports.is_empty();
        let waiting = save.retry_after.is_some_and(|at| Instant::now() < at);
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
        let client = Client::new(&record.address, token);
        let name = self.document.name.clone();
        let slot = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        std::thread::spawn(move || {
            let outcome = send(&client, &link, &name, &text, &document_path);
            *filled.lock().expect("server save") = Some(outcome);
        });
        save.running = Some(slot);
        save.state = Some(SaveState::Saving);
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
            let outcome = exports
                .iter()
                .try_for_each(|path| send_export(&client, &link, path));
            *filled.lock().expect("export save") = Some(outcome);
        });
        self.server_save.exporting = Some(slot);
        self.tell_export("Saving the export to the project…");
    }

    /// An export finished: saves it to the project too, when the picture is
    /// on a server and the person asked for that.
    pub(super) fn export_finished(&mut self, path: &Path) {
        if self.server_save.export_to_project && self.current_link().is_some() {
            self.server_save.exports.push(path.to_path_buf());
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
}

/// Fetches a document for this computer: its newest revision written to
/// `document_path`, with its edited file beside it, and the link. The
/// copy here is kept when it holds edits not yet sent, unless `discard`.
fn fetch(
    client: &Client,
    target: &super::open::TargetLink,
    id: &str,
    document_path: &Path,
    discard: bool,
) -> Result<(), Failure> {
    let record = client.document_record(&target.database, id)?;
    let revision = record
        .current()
        .map(|current| current.revision)
        .ok_or_else(|| Failure::Failed("The document has no revision".to_string()))?;
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
        let text = client.revision_text(&target.database, id, revision)?;
        let text = localize(client, &target.database, &record, &text, document_path)?;
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

/// A document of a target, made ready to open: the picture's name, once
/// its document and link are written.
pub(super) type Fetching = Arc<Mutex<Option<Result<String, Failure>>>>;

impl Studio {
    /// Opens one of a target's documents on a server: the document is
    /// fetched, then the picture is prepared as a link prepares it.
    pub(super) fn open_server_document(
        &mut self,
        target: super::open::TargetLink,
        which: Which,
        discard: bool,
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
                let id = match which {
                    Which::Document(id) => Some(id),
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
                    return Ok(name);
                };
                let name = format!("{base}-{}", &id.replace('-', "")[..8.min(id.len())]);
                fetch(
                    &client,
                    &target,
                    &id,
                    &folder.join(format!("{name}.json")),
                    discard,
                )?;
                Ok(name)
            })();
            *filled.lock().expect("fetching") = Some(outcome);
        });
        self.servers.fetching = Some((fetching_target, token, slot));
        self.say("Fetching the document from the server…");
    }

    /// Prepares a fetched document's picture once it is here.
    pub(super) fn follow_fetching(&mut self) {
        let Some((_, _, slot)) = &self.servers.fetching else {
            return;
        };
        let Some(outcome) = slot.lock().expect("fetching").take() else {
            return;
        };
        let Some((target, token, _)) = self.servers.fetching.take() else {
            return;
        };
        match outcome {
            Ok(name) => self.open_target_named(&target, token, &name),
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
            json!([{"filter_name":"Ha","job_id":"j","group_index":0,"artifact_revision":"r"}])
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
        let record = DocumentRecord {
            id: "a".to_string(),
            name: "Iris".to_string(),
            revisions: Vec::new(),
        };
        // No request is made for an exported junction.
        let client = Client::new("http://127.0.0.1:9", None);
        let text = localize(
            &client,
            "main",
            &record,
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
