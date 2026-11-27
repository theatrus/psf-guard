//! Undo, Redo and the History panel: the document as it was before each
//! edit, and each edit in a few words.

use super::Studio;
use crate::studio_model::{Document, Row};
use std::time::{Duration, Instant};

/// Edits closer together than this are one edit, so a slider dragged
/// across its range is undone at once.
const MERGE: Duration = Duration::from_millis(700);

/// The most edits Undo goes back through.
const DEPTH: usize = 200;

/// One edit: the document's text on one side of it, and the edit in a few
/// words.
struct Edit {
    text: String,
    words: String,
}

/// The document's edits, kept as the document's text before each.
#[derive(Default)]
pub(super) struct History {
    /// The document before each edit, the last edit last.
    undo: Vec<Edit>,
    /// The document after each edit undone, the last undone last.
    redo: Vec<Edit>,
    /// The document as it is now.
    current: String,
    /// When the document last changed, while a change may still join it.
    changed_at: Option<Instant>,
}

impl History {
    /// Starts again from `document`, with nothing to undo.
    pub(super) fn reset(&mut self, document: String) {
        *self = Self {
            current: document,
            ..Self::default()
        };
    }

    /// Records the document as it is `now`. A change soon after the last
    /// one joins it. `describe` words the edit from the document's text
    /// before it and after it. Returns whether the document changed.
    pub(super) fn note(
        &mut self,
        now: String,
        at: Instant,
        describe: impl FnOnce(&str, &str) -> String,
    ) -> bool {
        if now == self.current {
            return false;
        }
        let joins = self
            .changed_at
            .is_some_and(|last| at.saturating_duration_since(last) < MERGE)
            && !self.undo.is_empty();
        let before = std::mem::replace(&mut self.current, now);
        if joins {
            let last = self.undo.last_mut().expect("an edit to join");
            last.words = describe(&last.text, &self.current);
        } else {
            let words = describe(&before, &self.current);
            self.undo.push(Edit {
                text: before,
                words,
            });
            if self.undo.len() > DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.changed_at = Some(at);
        true
    }

    /// The document before the last edit, which becomes the document.
    pub(super) fn undo(&mut self) -> Option<&str> {
        let Edit { text, words } = self.undo.pop()?;
        let after = std::mem::replace(&mut self.current, text);
        self.redo.push(Edit { text: after, words });
        self.changed_at = None;
        Some(&self.current)
    }

    /// The document after the last edit undone, which becomes the
    /// document.
    pub(super) fn redo(&mut self) -> Option<&str> {
        let Edit { text, words } = self.redo.pop()?;
        let before = std::mem::replace(&mut self.current, text);
        self.undo.push(Edit {
            text: before,
            words,
        });
        self.changed_at = None;
        Some(&self.current)
    }

    /// Every edit in a few words, oldest first: those done, then those
    /// undone that Redo makes again. The document is after the last done.
    pub(super) fn words(&self) -> (Vec<&str>, Vec<&str>) {
        (
            self.undo.iter().map(|edit| edit.words.as_str()).collect(),
            self.redo
                .iter()
                .rev()
                .map(|edit| edit.words.as_str())
                .collect(),
        )
    }

    /// [`Self::note`] with every edit worded alike, for the tests.
    #[cfg(test)]
    fn note_plain(&mut self, now: String, at: Instant) -> bool {
        self.note(now, at, |_, after| format!("to {after}"))
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

impl Studio {
    /// Whether Redo, or else Undo, has an edit to act on.
    pub(super) fn history_can(&self, redo: bool) -> bool {
        if redo {
            self.history.can_redo()
        } else {
            self.history.can_undo()
        }
    }

    /// Records the document in the history after an edit.
    pub(super) fn note_edit(&mut self) {
        let now = self.document.to_json();
        self.history.note(now, Instant::now(), |before, after| {
            match (Document::from_json(before), Document::from_json(after)) {
                (Ok(before), Ok(after)) => crate::studio_model::describe_edit(&before, &after),
                _ => "Changed the document".to_string(),
            }
        });
    }

    /// Goes to the document after the edit at `position` in the History
    /// panel, 0 being the document as opened: undoing back to it, or
    /// redoing up to it. The edits after it stay to be redone.
    pub(super) fn go_to_history(&mut self, position: usize) {
        self.note_edit();
        let mut text = None;
        while self.history.undo.len() > position {
            text = self.history.undo().map(str::to_string);
        }
        while self.history.undo.len() < position {
            let Some(next) = self.history.redo().map(str::to_string) else {
                break;
            };
            text = Some(next);
        }
        if let Some(text) = text {
            self.restore(&text);
        }
    }

    /// Puts the document back as it was before the last edit.
    pub(super) fn undo(&mut self) {
        self.note_edit();
        let Some(text) = self.history.undo().map(str::to_string) else {
            return;
        };
        self.restore(&text);
    }

    /// Makes the last edit undone again.
    pub(super) fn redo(&mut self) {
        self.note_edit();
        let Some(text) = self.history.redo().map(str::to_string) else {
            return;
        };
        self.restore(&text);
    }

    fn restore(&mut self, text: &str) {
        if let Ok(document) = Document::from_json(text) {
            self.document = document;
            self.keep_selection();
            self.refresh_canvas();
        }
    }

    /// Selects what is left of the selected row when the document no longer
    /// has it: its input or layer, else the combined picture.
    pub(super) fn keep_selection(&mut self) {
        let has_step = |document: &Document, step: uuid::Uuid| document.step(step).is_some();
        let has_layer = |document: &Document, layer: uuid::Uuid| {
            document.layers().iter().any(|other| other.id == layer)
        };
        let channels = self.document.channels();
        let kept = match &self.selected {
            Row::Input(channel) => channels.contains(channel).then(|| self.selected.clone()),
            Row::InputStep { channel, step } => {
                if has_step(&self.document, *step) {
                    Some(self.selected.clone())
                } else {
                    channels
                        .contains(channel)
                        .then(|| Row::Input(channel.clone()))
                }
            }
            Row::Layer(layer) => has_layer(&self.document, *layer).then(|| self.selected.clone()),
            Row::LayerStep { layer, step } => {
                if self.document.layer_of_step(*step) == Some(*layer) {
                    Some(self.selected.clone())
                } else {
                    has_layer(&self.document, *layer).then_some(Row::Layer(*layer))
                }
            }
            Row::Mask(mask) => self.document.mask(*mask).map(|_| self.selected.clone()),
            Row::Finish(step) => has_step(&self.document, *step).then(|| self.selected.clone()),
            Row::Combined | Row::Final => Some(self.selected.clone()),
        };
        self.selected = kept.unwrap_or(Row::Combined);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_undo_and_redo_in_turn_and_a_drag_is_one_edit() {
        let start = Instant::now();
        let mut history = History::default();
        history.reset("a".to_string());
        assert!(!history.can_undo());
        assert!(!history.note_plain("a".to_string(), start));
        assert!(history.note_plain("b".to_string(), start));
        // A drag: changes in quick succession join the first.
        for (at, text) in [(100, "c"), (200, "d"), (300, "e")] {
            assert!(history.note_plain(text.to_string(), start + Duration::from_millis(at)));
        }
        // A later change is an edit of its own.
        assert!(history.note_plain("f".to_string(), start + Duration::from_secs(2)));
        assert_eq!(history.undo(), Some("e"));
        assert_eq!(history.undo(), Some("a"));
        assert_eq!(history.undo(), None);
        assert_eq!(history.redo(), Some("e"));
        // An edit after an undo drops what could be redone.
        assert!(history.note_plain("g".to_string(), start + Duration::from_secs(5)));
        assert!(!history.can_redo());
        assert_eq!(history.undo(), Some("e"));
        assert!(history.can_redo());
    }

    #[test]
    fn the_history_keeps_its_depth() {
        let start = Instant::now();
        let mut history = History::default();
        history.reset("0".to_string());
        for edit in 1..=(DEPTH + 10) {
            history.note_plain(edit.to_string(), start + Duration::from_secs(edit as u64));
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, DEPTH);
    }

    #[test]
    fn the_panel_lists_edits_done_then_undone() {
        let start = Instant::now();
        let mut history = History::default();
        history.reset("a".to_string());
        for (at, text) in [(0, "b"), (2, "c"), (4, "d")] {
            history.note_plain(text.to_string(), start + Duration::from_secs(at));
        }
        assert_eq!(history.words(), (vec!["to b", "to c", "to d"], vec![]));
        history.undo();
        history.undo();
        // Undone edits come after, in the order Redo makes them again.
        assert_eq!(history.words(), (vec!["to b"], vec!["to c", "to d"]));
        history.redo();
        assert_eq!(history.words(), (vec!["to b", "to c"], vec!["to d"]));
    }
}
