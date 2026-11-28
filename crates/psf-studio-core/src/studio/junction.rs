use super::StudioError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

/// What identifies one state of a file: enough to tell a real edit from a
/// file that was only touched or rewritten unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    /// The file's size in bytes.
    pub bytes: u64,
    /// The file's modification time, in nanoseconds since 1970.
    pub modified_nanos: u64,
    /// The SHA-256 hash of the file's content, in hexadecimal.
    pub sha256: String,
}

fn file_error(path: &Path) -> impl FnOnce(std::io::Error) -> StudioError + '_ {
    move |source| StudioError::File {
        path: path.to_path_buf(),
        source,
    }
}

/// The file's size and modification time, without reading it.
fn size_and_time(path: &Path) -> Result<(u64, u64), StudioError> {
    let metadata = std::fs::metadata(path).map_err(file_error(path))?;
    let modified = metadata
        .modified()
        .map_err(file_error(path))?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    Ok((metadata.len(), modified))
}

impl FileStamp {
    /// Reads the file once, in pieces, to stamp it. An intermediate image
    /// can be many gigabytes, so it is never held whole.
    pub fn of(path: &Path) -> Result<Self, StudioError> {
        let (bytes, modified_nanos) = size_and_time(path)?;
        let mut file = std::fs::File::open(path).map_err(file_error(path))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer).map_err(file_error(path))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let sha256 = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            bytes,
            modified_nanos,
            sha256,
        })
    }
}

/// What stands at the end of a stage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum JunctionState {
    /// Later stages read the stage's own result.
    #[default]
    Computed,
    /// The result was written out for an external editor, and the file is
    /// still as Studio wrote it.
    Exported {
        /// The exported file.
        path: PathBuf,
        /// The file as Studio wrote it.
        exported: FileStamp,
        /// A key for everything before the junction when the file was
        /// written.
        upstream: String,
    },
    /// The file came back changed. Later stages read from it.
    Edited {
        /// The edited file.
        path: PathBuf,
        /// The file as Studio wrote it.
        exported: FileStamp,
        /// The file as the editor saved it.
        edited: FileStamp,
        /// A key for everything before the junction when the file was
        /// written.
        upstream: String,
    },
}

/// What a junction means for the document now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JunctionStatus {
    /// Later stages read the stage's own result.
    Computed,
    /// A file is out with an editor and has not changed yet.
    WaitingForEdit,
    /// Later stages read an edited file made from the current result.
    Edited,
    /// Later stages read an edited file, and a step before the junction has
    /// changed since the file was written. The edit does not include that
    /// change.
    EditedFromOlderResult,
    /// A file is out with an editor, and a step before the junction has
    /// changed since it was written.
    WaitingOnOlderResult,
}

/// The end of a stage, where its result may be replaced by a file corrected
/// in an external editor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Junction {
    /// The junction's state.
    #[serde(flatten)]
    pub state: JunctionState,
}

impl Junction {
    /// Records that the stage's result was written to `path` for an editor.
    /// `upstream` is a key for everything before the junction. The caller
    /// writes the file first.
    pub fn exported(&mut self, path: &Path, upstream: &str) -> Result<(), StudioError> {
        self.state = JunctionState::Exported {
            path: path.to_path_buf(),
            exported: FileStamp::of(path)?,
            upstream: upstream.to_string(),
        };
        Ok(())
    }

    /// Looks at the exported file again and returns whether an edit has
    /// arrived since the last look.
    ///
    /// The file is hashed only when its size or time has changed. A file
    /// rewritten with the same content is not an edit. A file edited back to
    /// exactly what Studio wrote returns the junction to waiting.
    pub fn check_for_edit(&mut self) -> Result<bool, StudioError> {
        let (path, exported, known, upstream) = match &self.state {
            JunctionState::Computed => return Ok(false),
            JunctionState::Exported {
                path,
                exported,
                upstream,
            } => (path, exported, exported, upstream),
            JunctionState::Edited {
                path,
                exported,
                edited,
                upstream,
            } => (path, exported, edited, upstream),
        };
        let (bytes, modified_nanos) = size_and_time(path)?;
        if (bytes, modified_nanos) == (known.bytes, known.modified_nanos) {
            return Ok(false);
        }
        let now = FileStamp::of(path)?;
        let changed = now.sha256 != known.sha256;
        self.state = if now.sha256 == exported.sha256 {
            JunctionState::Exported {
                path: path.clone(),
                exported: now,
                upstream: upstream.clone(),
            }
        } else {
            JunctionState::Edited {
                path: path.clone(),
                exported: exported.clone(),
                edited: now,
                upstream: upstream.clone(),
            }
        };
        Ok(changed)
    }

    /// Goes back to the stage's own result. The file is left where it is.
    pub fn use_computed(&mut self) {
        self.state = JunctionState::Computed;
    }

    /// The file later stages read from, when an edit has replaced the
    /// stage's result.
    pub fn edited_file(&self) -> Option<&Path> {
        match &self.state {
            JunctionState::Edited { path, .. } => Some(path),
            _ => None,
        }
    }

    /// What the junction means now, given the current key for everything
    /// before it.
    pub fn status(&self, current_upstream: &str) -> JunctionStatus {
        match &self.state {
            JunctionState::Computed => JunctionStatus::Computed,
            JunctionState::Exported { upstream, .. } if upstream == current_upstream => {
                JunctionStatus::WaitingForEdit
            }
            JunctionState::Exported { .. } => JunctionStatus::WaitingOnOlderResult,
            JunctionState::Edited { upstream, .. } if upstream == current_upstream => {
                JunctionStatus::Edited
            }
            JunctionState::Edited { .. } => JunctionStatus::EditedFromOlderResult,
        }
    }
}

/// The command that opens `file` in an external editor.
///
/// `editor` is the application to use: on macOS an application's name, such
/// as "Adobe Photoshop 2026", and elsewhere a program's path. With `None`
/// the system's default application for the file opens it. The command
/// returns once the editor has been asked to open the file; Studio does not
/// wait for the editor to quit.
pub fn open_in_editor_command(editor: Option<&str>, file: &Path) -> Command {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        if let Some(editor) = editor {
            command.arg("-a").arg(editor);
        }
        command.arg(file);
        command
    } else if let Some(editor) = editor {
        let mut command = Command::new(editor);
        command.arg(file);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        // The empty string is the window title `start` expects first.
        command.args(["/C", "start", ""]).arg(file);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(file);
        command
    }
}
