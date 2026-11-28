//! PSF Studio documents saved to a database: each document's revisions, the
//! files they use, and the exports saved from them.
//!
//! The store keeps what a person made and Studio cannot make again: the
//! document, the file an external editor saved, exports and a preview. It
//! keeps none of what Studio computes from the document, such as tile
//! stores, stacks or tool results.
//!
//! A database's documents sit beside the database registry, under
//! `studio-documents/<database id>/`, not in a cache directory, so cache
//! cleanup never removes them:
//!
//! ```text
//! studio-documents/<database id>/
//!     <document id>/record.json        the record and its revision list
//!     <document id>/revisions/<n>.json each revision's document
//!     files/<sha256>                   files, stored once by content
//! ```
//!
//! Every write goes to a temporary file that is renamed into place.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The revisions a document keeps, besides those an export refers to.
pub const KEPT_REVISIONS: usize = 50;
/// The largest document a revision may hold.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
/// The largest file: an export of a large mosaic at 32 bits.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// How long a file no revision uses yet is kept: the time between
/// uploading a file and saving the revision that uses it.
pub const UNUSED_FILE_SECONDS: i64 = 60 * 60;
const MAX_NAME_CHARS: usize = 120;

/// A channel stack a document was made from, as the color stack list
/// identifies it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackFingerprint {
    pub filter_name: String,
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
    /// The frame the stack was registered to. Masks, heals and the crop
    /// are in its pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_image_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<usize>,
}

/// A file a revision uses, by what it is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileUse {
    /// What the file is for: `edited` for the file an external editor
    /// saved at the Finish junction.
    pub role: String,
    pub sha256: String,
    /// The file's name, for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevisionEntry {
    pub revision: u32,
    pub saved_at: i64,
    pub saved_by: String,
    /// The computer the revision was saved from, as PSF Studio gives its
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_from: Option<String>,
    #[serde(default)]
    pub files: Vec<FileUse>,
    /// A small image of the picture, when one was saved with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// An export saved to the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportEntry {
    pub sha256: String,
    /// The revision the export was made from.
    pub revision: u32,
    pub name: String,
    pub format: String,
    pub size: u64,
    pub saved_at: i64,
    pub saved_by: String,
}

/// A document's record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: String,
    pub name: String,
    pub project_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i32>,
    /// The channel stacks the document was made from.
    #[serde(default)]
    pub stacks: Vec<StackFingerprint>,
    /// The color card the document started from, as PSF Guard records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Value>,
    pub created_at: i64,
    pub created_by: String,
    /// Oldest first. Never empty.
    pub revisions: Vec<RevisionEntry>,
    #[serde(default)]
    pub exports: Vec<ExportEntry>,
}

impl DocumentRecord {
    pub fn current(&self) -> &RevisionEntry {
        self.revisions.last().expect("a document has a revision")
    }

    /// Every file the record uses.
    fn files(&self) -> impl Iterator<Item = &str> {
        self.revisions
            .iter()
            .flat_map(|revision| {
                revision
                    .files
                    .iter()
                    .map(|file| file.sha256.as_str())
                    .chain(revision.preview.as_deref())
            })
            .chain(self.exports.iter().map(|export| export.sha256.as_str()))
    }
}

/// A document as a list shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DocumentSummary {
    pub id: String,
    pub name: String,
    pub project_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i32>,
    pub revision: u32,
    pub saved_at: i64,
    pub saved_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Exports saved to the project, newest first.
    pub exports: Vec<ExportEntry>,
}

impl From<&DocumentRecord> for DocumentSummary {
    fn from(record: &DocumentRecord) -> Self {
        let current = record.current();
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            project_id: record.project_id,
            target_id: record.target_id,
            revision: current.revision,
            saved_at: current.saved_at,
            saved_by: current.saved_by.clone(),
            saved_from: current.saved_from.clone(),
            preview: record
                .revisions
                .iter()
                .rev()
                .find_map(|revision| revision.preview.clone()),
            exports: record.exports.iter().rev().cloned().collect(),
        }
    }
}

/// A new document and its first revision.
#[derive(Clone, Debug, Deserialize)]
pub struct NewDocument {
    pub name: String,
    pub project_id: i32,
    #[serde(default)]
    pub target_id: Option<i32>,
    #[serde(default)]
    pub stacks: Vec<StackFingerprint>,
    #[serde(default)]
    pub card: Option<Value>,
    pub document: Value,
    #[serde(default)]
    pub files: Vec<FileUse>,
    #[serde(default)]
    pub preview: Option<String>,
    /// The computer the document is saved from.
    #[serde(default)]
    pub saved_from: Option<String>,
}

/// A save of an existing document.
#[derive(Clone, Debug, Deserialize)]
pub struct Save {
    /// The revision the save started from. A save that started from an
    /// older revision than the current one is a conflict.
    pub base_revision: u32,
    pub document: Value,
    #[serde(default)]
    pub files: Vec<FileUse>,
    #[serde(default)]
    pub preview: Option<String>,
    /// A new name, when the document was renamed.
    #[serde(default)]
    pub name: Option<String>,
    /// The stacks, when the document was opened with newer ones.
    #[serde(default)]
    pub stacks: Option<Vec<StackFingerprint>>,
    /// The computer the document is saved from.
    #[serde(default)]
    pub saved_from: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Saved {
    Saved(DocumentRecord),
    /// Another save arrived first: the record as it is.
    Conflict(DocumentRecord),
}

/// An export recorded against a revision. The file is uploaded first.
#[derive(Clone, Debug, Deserialize)]
pub struct NewExport {
    pub sha256: String,
    pub revision: u32,
    pub name: String,
    pub format: String,
    /// A small image of the export, uploaded first, which becomes the
    /// preview of the revision it came from.
    #[serde(default)]
    pub preview: Option<String>,
}

/// One database's Studio documents.
pub struct StudioDocuments {
    root: PathBuf,
}

fn valid_sha256(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
}

/// The most characters kept of a computer's name.
const MAX_COMPUTER_CHARS: usize = 64;

/// A computer's name as a revision keeps it: without control characters,
/// trimmed, and cut to `MAX_COMPUTER_CHARS`. `None` when nothing is left.
fn clean_computer(name: Option<&str>) -> Option<String> {
    let name: String = name?
        .chars()
        .filter(|letter| !letter.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_COMPUTER_CHARS)
        .collect();
    (!name.is_empty()).then_some(name)
}

fn clean_name(name: &str) -> Result<String> {
    let name: String = name
        .chars()
        .filter(|letter| !letter.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        bail!("a document needs a name");
    }
    if name.chars().count() > MAX_NAME_CHARS {
        bail!("a document's name is at most {MAX_NAME_CHARS} characters");
    }
    Ok(name)
}

/// The document as text, once it reads as a Studio document.
fn document_text(document: &Value) -> Result<String> {
    let text = serde_json::to_string(document)?;
    if text.len() > MAX_DOCUMENT_BYTES {
        bail!(
            "the document is larger than {} MB",
            MAX_DOCUMENT_BYTES / 1024 / 1024
        );
    }
    crate::studio::Document::from_json(&text)
        .map_err(|error| anyhow!("this is not a PSF Studio document: {error}"))?;
    Ok(text)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let folder = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(folder).with_context(|| format!("creating {}", folder.display()))?;
    let temporary = folder.join(format!(
        ".{}.{}.saving",
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().to_string()),
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = std::fs::File::create(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))
}

impl StudioDocuments {
    /// The documents of `database`, beside the database registry at
    /// `registry_path`.
    pub fn for_database(registry_path: &Path, database: &str) -> Result<Self> {
        if database.is_empty()
            || !database
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            bail!("{database:?} is not a database id");
        }
        let folder = registry_path
            .parent()
            .ok_or_else(|| anyhow!("the database registry has no folder"))?;
        Ok(Self {
            root: folder.join("studio-documents").join(database),
        })
    }

    fn document_folder(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            bail!("{id:?} is not a document id");
        }
        Ok(self.root.join(id))
    }

    fn files_folder(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Where a file is kept, whether or not it is there.
    pub fn file_path(&self, sha256: &str) -> Result<PathBuf> {
        if !valid_sha256(sha256) {
            bail!("{sha256:?} is not a SHA-256 in lowercase hex");
        }
        Ok(self.files_folder().join(sha256))
    }

    pub fn has_file(&self, sha256: &str) -> Result<bool> {
        Ok(self.file_path(sha256)?.is_file())
    }

    /// A new temporary file in the files folder, for an upload.
    pub fn incoming_file(&self) -> Result<PathBuf> {
        let folder = self.files_folder();
        std::fs::create_dir_all(&folder)
            .with_context(|| format!("creating {}", folder.display()))?;
        Ok(folder.join(format!(".incoming-{}", uuid::Uuid::new_v4().simple())))
    }

    /// Keeps an uploaded file under its hash, which the caller computed
    /// while it was written.
    pub fn keep_file(&self, incoming: &Path, sha256: &str) -> Result<()> {
        let path = self.file_path(sha256)?;
        std::fs::rename(incoming, &path).with_context(|| format!("keeping {}", path.display()))
    }

    pub fn record(&self, id: &str) -> Result<Option<DocumentRecord>> {
        let path = self.document_folder(id)?.join("record.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(
                serde_json::from_str(&text)
                    .with_context(|| format!("reading {}", path.display()))?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn records(&self) -> Result<Vec<DocumentRecord>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("listing Studio documents"),
        };
        let mut records = Vec::new();
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().to_string();
            if valid_id(&name)
                && let Some(record) = self.record(&name)?
            {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// The documents of a project, or of one target, most recently saved
    /// first.
    pub fn list(&self, project: Option<i32>, target: Option<i32>) -> Result<Vec<DocumentSummary>> {
        let mut summaries: Vec<DocumentSummary> = self
            .records()?
            .iter()
            .filter(|record| project.is_none_or(|project| record.project_id == project))
            .filter(|record| target.is_none_or(|target| record.target_id == Some(target)))
            .map(DocumentSummary::from)
            .collect();
        summaries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
        Ok(summaries)
    }

    /// A revision's document, as text.
    pub fn revision(&self, id: &str, revision: u32) -> Result<Option<String>> {
        let path = self
            .document_folder(id)?
            .join("revisions")
            .join(format!("{revision}.json"));
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn check_files(&self, files: &[FileUse], preview: Option<&str>) -> Result<()> {
        for sha in files.iter().map(|file| file.sha256.as_str()).chain(preview) {
            if !self.has_file(sha)? {
                bail!("the file {sha} is not on the server: upload it first");
            }
        }
        Ok(())
    }

    fn write_record(&self, record: &DocumentRecord) -> Result<()> {
        let path = self.document_folder(&record.id)?.join("record.json");
        write_atomically(&path, serde_json::to_string_pretty(record)?.as_bytes())
    }

    fn write_revision(&self, id: &str, revision: u32, text: &str) -> Result<()> {
        let path = self
            .document_folder(id)?
            .join("revisions")
            .join(format!("{revision}.json"));
        write_atomically(&path, text.as_bytes())
    }

    pub fn create(&self, new: NewDocument, by: &str, now: i64) -> Result<DocumentRecord> {
        let name = clean_name(&new.name)?;
        let text = document_text(&new.document)?;
        self.check_files(&new.files, new.preview.as_deref())?;
        let record = DocumentRecord {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            name,
            project_id: new.project_id,
            target_id: new.target_id,
            stacks: new.stacks,
            card: new.card,
            created_at: now,
            created_by: by.to_string(),
            revisions: vec![RevisionEntry {
                revision: 1,
                saved_at: now,
                saved_by: by.to_string(),
                saved_from: clean_computer(new.saved_from.as_deref()),
                files: new.files,
                preview: new.preview,
            }],
            exports: Vec::new(),
        };
        self.write_revision(&record.id, 1, &text)?;
        self.write_record(&record)?;
        Ok(record)
    }

    /// Saves a new revision, unless another save arrived first.
    pub fn save(&self, id: &str, save: Save, by: &str, now: i64) -> Result<Option<Saved>> {
        let Some(mut record) = self.record(id)? else {
            return Ok(None);
        };
        if record.current().revision != save.base_revision {
            return Ok(Some(Saved::Conflict(record)));
        }
        let text = document_text(&save.document)?;
        self.check_files(&save.files, save.preview.as_deref())?;
        if let Some(name) = &save.name {
            record.name = clean_name(name)?;
        }
        if let Some(stacks) = save.stacks {
            record.stacks = stacks;
        }
        let revision = save.base_revision + 1;
        self.write_revision(id, revision, &text)?;
        record.revisions.push(RevisionEntry {
            revision,
            saved_at: now,
            saved_by: by.to_string(),
            saved_from: clean_computer(save.saved_from.as_deref()),
            files: save.files,
            preview: save.preview,
        });
        let dropped = self.prune(&mut record);
        self.write_record(&record)?;
        for old in dropped {
            let _ = std::fs::remove_file(
                self.document_folder(id)?
                    .join("revisions")
                    .join(format!("{old}.json")),
            );
        }
        self.collect_garbage(now)?;
        Ok(Some(Saved::Saved(record)))
    }

    /// Drops revisions beyond the kept number, except those an export was
    /// made from. Returns the dropped revision numbers.
    fn prune(&self, record: &mut DocumentRecord) -> Vec<u32> {
        let exported: HashSet<u32> = record
            .exports
            .iter()
            .map(|export| export.revision)
            .collect();
        let newest_kept = record.revisions.len().saturating_sub(KEPT_REVISIONS);
        let mut dropped = Vec::new();
        let mut kept = Vec::new();
        for (at, revision) in record.revisions.drain(..).enumerate() {
            if at >= newest_kept || exported.contains(&revision.revision) {
                kept.push(revision);
            } else {
                dropped.push(revision.revision);
            }
        }
        record.revisions = kept;
        dropped
    }

    pub fn add_export(
        &self,
        id: &str,
        export: NewExport,
        by: &str,
        now: i64,
    ) -> Result<Option<DocumentRecord>> {
        let Some(mut record) = self.record(id)? else {
            return Ok(None);
        };
        let Some(at) = record
            .revisions
            .iter()
            .position(|revision| revision.revision == export.revision)
        else {
            bail!("the document has no revision {}", export.revision);
        };
        if let Some(preview) = &export.preview {
            if !self.has_file(preview)? {
                bail!("the file {preview} is not on the server: upload it first");
            }
            record.revisions[at].preview = Some(preview.clone());
        }
        let path = self.file_path(&export.sha256)?;
        let size = std::fs::metadata(&path)
            .map_err(|_| {
                anyhow!(
                    "the file {} is not on the server: upload it first",
                    export.sha256
                )
            })?
            .len();
        let name = clean_name(&export.name)?;
        let format = export.format.trim().to_ascii_lowercase();
        if format.is_empty()
            || format.len() > 8
            || !format.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            bail!("{format:?} is not an export format");
        }
        record.exports.push(ExportEntry {
            sha256: export.sha256,
            revision: export.revision,
            name,
            format,
            size,
            saved_at: now,
            saved_by: by.to_string(),
        });
        self.write_record(&record)?;
        Ok(Some(record))
    }

    /// Deletes a document, its revisions, and the files no other document
    /// uses. Returns whether there was one.
    pub fn delete(&self, id: &str, now: i64) -> Result<bool> {
        let Some(record) = self.record(id)? else {
            return Ok(false);
        };
        std::fs::remove_dir_all(self.document_folder(id)?)
            .with_context(|| format!("deleting the document {id}"))?;
        let used = self.used_files()?;
        for sha in record.files() {
            if !used.contains(sha) {
                let _ = std::fs::remove_file(self.file_path(sha)?);
            }
        }
        self.collect_garbage(now)?;
        Ok(true)
    }

    fn used_files(&self) -> Result<HashSet<String>> {
        Ok(self
            .records()?
            .iter()
            .flat_map(|record| record.files().map(str::to_string).collect::<Vec<_>>())
            .collect())
    }

    /// Removes files no document uses that were uploaded more than an hour
    /// ago, and uploads that never finished.
    pub fn collect_garbage(&self, now: i64) -> Result<()> {
        let entries = match std::fs::read_dir(self.files_folder()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("listing Studio files"),
        };
        let used = self.used_files()?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if used.contains(&name) {
                continue;
            }
            let age = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| now - since.as_secs() as i64);
            if age > UNUSED_FILE_SECONDS {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Value {
        serde_json::from_str(&crate::studio::Document::new("Iris").to_json()).unwrap()
    }

    fn store(folder: &tempfile::TempDir) -> StudioDocuments {
        StudioDocuments::for_database(&folder.path().join("config.json"), "main").unwrap()
    }

    fn put_file(store: &StudioDocuments, bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let sha: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let incoming = store.incoming_file().unwrap();
        std::fs::write(&incoming, bytes).unwrap();
        store.keep_file(&incoming, &sha).unwrap();
        sha
    }

    fn new_document(files: Vec<FileUse>) -> NewDocument {
        NewDocument {
            name: "Iris".to_string(),
            project_id: 11,
            target_id: Some(17),
            stacks: Vec::new(),
            card: None,
            document: document(),
            files,
            preview: None,
            saved_from: None,
        }
    }

    #[test]
    fn a_document_is_saved_as_revisions_and_a_stale_save_conflicts() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let created = store.create(new_document(Vec::new()), "ana", 100).unwrap();
        assert_eq!(created.current().revision, 1);
        let save = |base| Save {
            base_revision: base,
            document: document(),
            files: Vec::new(),
            preview: None,
            name: None,
            stacks: None,
            saved_from: None,
        };
        let Some(Saved::Saved(saved)) = store.save(&created.id, save(1), "ben", 200).unwrap()
        else {
            panic!("saved");
        };
        assert_eq!(saved.current().revision, 2);
        assert_eq!(saved.current().saved_by, "ben");
        // A save from revision 1 arrives after revision 2.
        assert!(matches!(
            store.save(&created.id, save(1), "ana", 300).unwrap(),
            Some(Saved::Conflict(record)) if record.current().revision == 2
        ));
        assert!(store.revision(&created.id, 2).unwrap().is_some());
        assert_eq!(store.list(Some(11), None).unwrap()[0].revision, 2);
        assert!(store.list(Some(12), None).unwrap().is_empty());
        assert!(store.list(None, Some(17)).unwrap().len() == 1);
    }

    #[test]
    fn a_revision_keeps_the_computer_it_was_saved_from() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let created = store
            .create(
                NewDocument {
                    saved_from: Some("  Studio\u{7}Mac ".to_string()),
                    ..new_document(Vec::new())
                },
                "ana",
                100,
            )
            .unwrap();
        assert_eq!(created.current().saved_from.as_deref(), Some("StudioMac"));
        let save = Save {
            base_revision: 1,
            document: document(),
            files: Vec::new(),
            preview: None,
            name: None,
            stacks: None,
            saved_from: Some("x".repeat(100)),
        };
        let Some(Saved::Saved(saved)) = store.save(&created.id, save, "ana", 200).unwrap() else {
            panic!("saved");
        };
        assert_eq!(
            saved.current().saved_from.as_ref().map(String::len),
            Some(64)
        );
        let listed = &store.list(Some(11), None).unwrap()[0];
        assert_eq!(listed.saved_from, saved.current().saved_from);
        // A save that gives no computer keeps none.
        let blank = Save {
            base_revision: 2,
            document: document(),
            files: Vec::new(),
            preview: None,
            name: None,
            stacks: None,
            saved_from: Some("   ".to_string()),
        };
        let Some(Saved::Saved(saved)) = store.save(&created.id, blank, "ana", 300).unwrap() else {
            panic!("saved");
        };
        assert_eq!(saved.current().saved_from, None);
    }

    #[test]
    fn only_a_studio_document_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let mut new = new_document(Vec::new());
        new.document = serde_json::json!({ "not": "a document" });
        assert!(store.create(new, "ana", 100).is_err());
        let mut new = new_document(Vec::new());
        new.name = "  ".to_string();
        assert!(store.create(new, "ana", 100).is_err());
    }

    #[test]
    fn a_revision_needs_its_files_on_the_server() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let missing = FileUse {
            role: "edited".to_string(),
            sha256: "a".repeat(64),
            name: None,
        };
        assert!(store
            .create(new_document(vec![missing]), "ana", 100)
            .is_err());
        let sha = put_file(&store, b"edited pixels");
        let created = store
            .create(
                new_document(vec![FileUse {
                    role: "edited".to_string(),
                    sha256: sha.clone(),
                    name: Some("Iris.edit.tif".to_string()),
                }]),
                "ana",
                100,
            )
            .unwrap();
        assert!(store.has_file(&sha).unwrap());
        assert!(store.file_path("../etc/passwd").is_err());
        assert!(store.record("../x").is_err());
        assert!(store.record(&created.id).unwrap().is_some());
    }

    #[test]
    fn old_revisions_go_but_an_exported_one_stays() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let created = store.create(new_document(Vec::new()), "ana", 100).unwrap();
        let export = put_file(&store, b"exported picture");
        store
            .add_export(
                &created.id,
                NewExport {
                    sha256: export.clone(),
                    revision: 1,
                    name: "Iris.tif".to_string(),
                    format: "TIF".to_string(),
                    preview: Some(export.clone()),
                },
                "ana",
                100,
            )
            .unwrap();
        for base in 1..=(KEPT_REVISIONS as u32 + 5) {
            store
                .save(
                    &created.id,
                    Save {
                        base_revision: base,
                        document: document(),
                        files: Vec::new(),
                        preview: None,
                        name: None,
                        stacks: None,
                        saved_from: None,
                    },
                    "ana",
                    100,
                )
                .unwrap();
        }
        let record = store.record(&created.id).unwrap().unwrap();
        assert_eq!(record.revisions.len(), KEPT_REVISIONS + 1);
        assert_eq!(
            record.revisions[0].revision, 1,
            "the exported revision stays"
        );
        assert!(store.revision(&created.id, 1).unwrap().is_some());
        assert!(store.revision(&created.id, 2).unwrap().is_none());
        assert_eq!(record.exports[0].format, "tif");
        assert_eq!(record.exports[0].size, 16);
        assert_eq!(
            record.revisions[0].preview.as_deref(),
            Some(export.as_str())
        );
        assert_eq!(
            store.list(None, None).unwrap()[0].preview.as_deref(),
            Some(export.as_str()),
            "the newest revision with a preview gives the list's"
        );
    }

    #[test]
    fn deleting_a_document_removes_the_files_only_it_uses() {
        let folder = tempfile::tempdir().unwrap();
        let store = store(&folder);
        let shared = put_file(&store, b"shared");
        let own = put_file(&store, b"own");
        let uses = |sha: &str| FileUse {
            role: "edited".to_string(),
            sha256: sha.to_string(),
            name: None,
        };
        let first = store
            .create(new_document(vec![uses(&shared), uses(&own)]), "ana", 100)
            .unwrap();
        store
            .create(new_document(vec![uses(&shared)]), "ana", 100)
            .unwrap();
        // Uploaded now, used by nothing yet: kept for the save that uses it.
        let pending = put_file(&store, b"pending");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(store.delete(&first.id, now).unwrap());
        assert!(!store.has_file(&own).unwrap());
        assert!(store.has_file(&shared).unwrap());
        assert!(store.has_file(&pending).unwrap());
        assert!(store.record(&first.id).unwrap().is_none());
        // An hour on, an unused file goes.
        store
            .collect_garbage(now + UNUSED_FILE_SECONDS + 1)
            .unwrap();
        assert!(!store.has_file(&pending).unwrap());
        assert!(store.has_file(&shared).unwrap());
        assert!(!store.delete(&first.id, now).unwrap());
    }

    #[test]
    fn a_database_id_cannot_leave_the_folder() {
        let registry = Path::new("/srv/psf-guard/config.json");
        assert!(StudioDocuments::for_database(registry, "../x").is_err());
        assert!(StudioDocuments::for_database(registry, "").is_err());
        assert!(StudioDocuments::for_database(registry, "db-1a2b").is_ok());
    }
}
