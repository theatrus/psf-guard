//! The round trip to another editor: the picture written out for an
//! editor such as Photoshop, and the file it saves read back in.
//!
//! **Open in external editor** exports the final picture as a 16-bit TIFF
//! beside the document, records the Finish stage's junction as exported,
//! and opens the file in its default app. Studio looks at the file about
//! once a second. When the editor saves a change, the junction is edited,
//! Studio reads the file, and the canvas and the export draw it in place of
//! Studio's picture, until **Use Studio's result** goes back.

use super::Studio;
use crate::derived::{DerivedKey, DerivedKind, DerivedPlane};
use crate::studio_model::{open_in_editor_command, Document, JunctionStatus, StageKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the file out with an editor is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// The edited file's planes, or why it could not be read.
type Read = Result<Vec<DerivedPlane>, String>;

/// The edited file as it is read on a thread of its own.
pub(super) struct EditorReading {
    path: PathBuf,
    done: Arc<Mutex<Option<Read>>>,
}

/// A key for everything before the Finish stage's junction: the document
/// with the junction set back, so the junction's own state does not count.
pub(super) fn editor_upstream(document: &Document) -> String {
    let mut document = document.clone();
    document.junction_mut(StageKind::Finish).use_computed();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&document.to_json(), &mut hasher);
    format!("{:016x}", std::hash::Hasher::finish(&hasher))
}

/// The file the picture is written to for an editor: beside the document,
/// or beside the store when the document has not been saved.
pub(super) fn editor_file(document: Option<&Path>, store: &Path, name: &str) -> PathBuf {
    let folder = document
        .and_then(Path::parent)
        .or_else(|| store.parent())
        .unwrap_or_else(|| Path::new("."));
    let stem = document.and_then(Path::file_stem).map_or_else(
        || name.to_string(),
        |stem| stem.to_string_lossy().to_string(),
    );
    folder.join(format!("{stem}.edit.tif"))
}

/// The edited file's channels as planes, one per channel of red, green and
/// blue, with the store's pyramid levels.
fn read_edited(path: &Path, levels: usize) -> Result<Vec<DerivedPlane>, String> {
    let image = seiza_image_io::open(path).map_err(|error| error.to_string())?;
    let channels = image.channels.max(1);
    let pixels = image.width * image.height;
    Ok((0..channels.min(3))
        .map(|channel| {
            let samples = (0..pixels)
                .map(|at| {
                    image
                        .samples
                        .get(at * channels + channel)
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect();
            DerivedPlane::of_samples(
                DerivedKey {
                    kind: DerivedKind::Edited {
                        channel: channel as u8,
                    },
                    bands: Vec::new(),
                    states: Vec::new(),
                    range: (0.0, 1.0),
                },
                image.width,
                image.height,
                samples,
                levels,
            )
        })
        .collect())
}

impl Studio {
    /// Exports the final picture for an editor, then opens it there.
    pub(super) fn open_in_editor(&mut self) {
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return;
        };
        let path = editor_file(self.document_path(), &store, &self.document.name);
        self.start_export_for(path, Some(editor_upstream(&self.document)));
    }

    /// Goes back to Studio's own picture; the edited file is left where it
    /// is.
    pub(super) fn use_studio_result(&mut self) {
        self.document.junction_mut(StageKind::Finish).use_computed();
        self.editor_reading = None;
        self.drop_edited_planes();
        self.refresh_canvas();
    }

    /// What the Finish stage's junction means now.
    pub(super) fn editor_status(&self) -> JunctionStatus {
        self.document
            .stage(StageKind::Finish)
            .junction
            .status(&editor_upstream(&self.document))
    }

    /// Records the file an export wrote for an editor and opens it there.
    pub(super) fn exported_for_editor(
        &mut self,
        path: &Path,
        upstream: &str,
    ) -> Result<(), String> {
        self.document
            .junction_mut(StageKind::Finish)
            .exported(path, upstream)
            .map_err(|error| error.to_string())?;
        open_in_editor_command(None, path)
            .spawn()
            .map_err(|error| format!("it could not be opened: {error}"))?;
        Ok(())
    }

    fn drop_edited_planes(&mut self) {
        let before = self.derived.len();
        self.derived
            .retain(|plane| !matches!(plane.key.kind, DerivedKind::Edited { .. }));
        if self.derived.len() != before {
            self.show_derived_planes();
        }
    }

    /// Looks at the file out with an editor, reads it when it has been
    /// edited, and takes in what was read.
    pub(super) fn follow_editor(&mut self) {
        if let Some(reading) = &self.editor_reading {
            let done = reading.done.lock().expect("edited file").take();
            if let Some(done) = done {
                self.editor_reading = None;
                match done {
                    Ok(planes) => {
                        self.drop_edited_planes();
                        self.derived.extend(planes.into_iter().map(Arc::new));
                        self.show_derived_planes();
                        self.tell_export("The edited file is in. The canvas shows it.");
                    }
                    Err(reason) => {
                        self.tell_export(&format!("The edited file was not read: {reason}"))
                    }
                }
                self.refresh_canvas();
            }
            return;
        }
        let junction = &self.document.stage(StageKind::Finish).junction;
        if junction.edited_file().is_none() && junction.state == Default::default() {
            return;
        }
        if self
            .editor_looked
            .is_some_and(|last| last.elapsed() < LOOK_EVERY)
        {
            return;
        }
        self.editor_looked = Some(Instant::now());
        let edited = self
            .document
            .junction_mut(StageKind::Finish)
            .check_for_edit()
            .unwrap_or(false);
        // An edit, or an edited file not read since the document opened.
        let unread = self
            .document
            .stage(StageKind::Finish)
            .junction
            .edited_file()
            .is_some()
            && !self
                .derived
                .iter()
                .any(|plane| matches!(plane.key.kind, DerivedKind::Edited { .. }));
        if !(edited || unread) {
            return;
        }
        let Some(path) = self
            .document
            .stage(StageKind::Finish)
            .junction
            .edited_file()
            .map(Path::to_path_buf)
        else {
            return;
        };
        let done = Arc::new(Mutex::new(None));
        let levels = usize::from(self.levels).max(1);
        let reading = EditorReading {
            path: path.clone(),
            done: done.clone(),
        };
        std::thread::spawn(move || {
            let read = read_edited(&path, levels);
            *done.lock().expect("edited file") = Some(read);
        });
        self.tell_export(&format!(
            "Reading {}…",
            reading
                .path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().to_string())
        ));
        self.editor_reading = Some(reading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_for_an_editor_sits_beside_the_document() {
        let store = Path::new("/data/iris.pgtiles");
        assert_eq!(
            editor_file(Some(Path::new("/work/Iris v2.json")), store, "Iris"),
            Path::new("/work/Iris v2.edit.tif")
        );
        assert_eq!(
            editor_file(None, store, "Iris"),
            Path::new("/data/Iris.edit.tif")
        );
    }
}
