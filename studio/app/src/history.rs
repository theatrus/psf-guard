//! Undo and Redo: the document as it was before each edit.

use super::Studio;
use crate::studio_model::{Document, Row};
use std::time::{Duration, Instant};

/// Edits closer together than this are one edit, so a slider dragged
/// across its range is undone at once.
const MERGE: Duration = Duration::from_millis(700);

/// The most edits Undo goes back through.
const DEPTH: usize = 200;

/// The document's edits, kept as the document's text before each.
#[derive(Default)]
pub(super) struct History {
    /// The document before each edit, the last edit last.
    undo: Vec<String>,
    /// The document after each edit undone, the last undone last.
    redo: Vec<String>,
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
    /// one joins it. Returns whether the document changed.
    pub(super) fn note(&mut self, now: String, at: Instant) -> bool {
        if now == self.current {
            return false;
        }
        let joins = self
            .changed_at
            .is_some_and(|last| at.saturating_duration_since(last) < MERGE)
            && !self.undo.is_empty();
        let before = std::mem::replace(&mut self.current, now);
        if !joins {
            self.undo.push(before);
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
        let before = self.undo.pop()?;
        self.redo.push(std::mem::replace(&mut self.current, before));
        self.changed_at = None;
        Some(&self.current)
    }

    /// The document after the last edit undone, which becomes the
    /// document.
    pub(super) fn redo(&mut self) -> Option<&str> {
        let after = self.redo.pop()?;
        self.undo.push(std::mem::replace(&mut self.current, after));
        self.changed_at = None;
        Some(&self.current)
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
        self.history.note(now, Instant::now());
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
        assert!(!history.note("a".to_string(), start));
        assert!(history.note("b".to_string(), start));
        // A drag: changes in quick succession join the first.
        for (at, text) in [(100, "c"), (200, "d"), (300, "e")] {
            assert!(history.note(text.to_string(), start + Duration::from_millis(at)));
        }
        // A later change is an edit of its own.
        assert!(history.note("f".to_string(), start + Duration::from_secs(2)));
        assert_eq!(history.undo(), Some("e"));
        assert_eq!(history.undo(), Some("a"));
        assert_eq!(history.undo(), None);
        assert_eq!(history.redo(), Some("e"));
        // An edit after an undo drops what could be redone.
        assert!(history.note("g".to_string(), start + Duration::from_secs(5)));
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
            history.note(edit.to_string(), start + Duration::from_secs(edit as u64));
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, DEPTH);
    }
}
