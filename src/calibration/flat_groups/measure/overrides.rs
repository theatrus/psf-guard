//! A person's choices about flat groups, applied after the measurements.
//!
//! - **Group overrides** split one family's groups at a session, even where
//!   the nights measure alike, or join two neighbouring groups across a
//!   split, even where they measure apart. They are keyed by the flats
//!   themselves (a session's first flat and its session key), never by row
//!   ids, so a regroup or a copy of the catalog finds them again.
//! - **Light choices** put one light before or after a change, or give it
//!   one group's flats, over its time, its imaging session and its pixels.
//!   They are keyed by the light's GUID and the change's key and window.
//!
//! A choice whose flats or change are gone stays recorded and shows as
//! orphaned; it is never applied to something it was not made for.

use super::model::{Book, Side};
use serde::{Deserialize, Serialize};

/// What a person did to one edge between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupChoice {
    /// A new group starts at the session.
    Split,
    /// The session's group and the one before it are one.
    Join,
}

impl GroupChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupChoice::Split => "split",
            GroupChoice::Join => "join",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "split" => Some(GroupChoice::Split),
            "join" => Some(GroupChoice::Join),
            _ => None,
        }
    }
}

/// One split or join as the catalog keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupOverrideRow {
    /// The first flat of the session that starts the later side: the edge.
    pub anchor_frame: String,
    pub session_key: String,
    /// The family's key when the choice was made.
    pub family_key: String,
    pub rig_uuid: String,
    /// `None` once reset: kept, with when, so a copy's older choice does
    /// not come back.
    pub choice: Option<GroupChoice>,
    /// For a join, the session on the earlier side.
    pub before_frame: Option<String>,
    pub before_session_key: Option<String>,
    /// The edge in words when it was made: "R at 104.0°, 2026-08-11 | 2026-08-12".
    pub label: String,
    pub chosen_at: i64,
}

/// Which side of a change, or which group, a person put a light on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightChoice {
    Before,
    After,
    Group,
}

impl LightChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            LightChoice::Before => "before",
            LightChoice::After => "after",
            LightChoice::Group => "group",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "before" => Some(LightChoice::Before),
            "after" => Some(LightChoice::After),
            "group" => Some(LightChoice::Group),
            _ => None,
        }
    }

    pub fn side(self) -> Option<Side> {
        match self {
            LightChoice::Before => Some(Side::Before),
            LightChoice::After => Some(Side::After),
            LightChoice::Group => None,
        }
    }
}

/// The key a light's group choice is kept under, beside its side choices.
pub const GROUP_CHOICE_KEY: &str = "group";

/// One light's choice as the catalog keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LightChoiceRow {
    pub light_guid: String,
    /// The change's key, or [`GROUP_CHOICE_KEY`].
    pub change_key: String,
    pub rig_uuid: String,
    /// The change's window when the choice was made: a change whose window
    /// later narrows, as new flats arrive, is found again by it.
    pub change_start: Option<i64>,
    pub change_end: Option<i64>,
    pub choice: Option<LightChoice>,
    /// For a group choice: a flat of a session of the group, and that
    /// session's key.
    pub group_frame: Option<String>,
    pub group_session_key: Option<String>,
    /// What the choice was about, in words, for when it is orphaned.
    pub label: String,
    pub chosen_at: i64,
}

/// A light's choices with what the book needs to find the light again: the
/// file name its frame is read from, its filter and capture time.
#[derive(Debug, Clone, PartialEq)]
pub struct LightChoiceEntry {
    pub light_id: i64,
    pub file_name: String,
    pub filter: Option<String>,
    pub at: Option<i64>,
    pub row: LightChoiceRow,
}

/// Where one group override stands in the book.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OverrideStatus {
    /// In force. `agrees` when the measurements alone group the same.
    Applied { agrees: bool },
    /// Its flats are gone, or no longer one family: kept, shown, unused.
    Orphaned { reason: String },
}

/// One group override as the book applied it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OverrideState {
    pub row: GroupOverrideRow,
    pub status: OverrideStatus,
    /// The family it applies to now, by index.
    pub family: Option<usize>,
    /// The sessions it names now: the one before (a join's) and the one
    /// the edge comes before.
    pub before: Option<usize>,
    pub after: Option<usize>,
    /// The edges it made (a split) or joined, in the book's boundaries.
    pub boundaries: Vec<usize>,
    /// A camera-side change it stands against, by key: both stay in force.
    pub conflict: Option<String>,
}

impl OverrideState {
    pub fn applied(&self) -> bool {
        matches!(self.status, OverrideStatus::Applied { .. })
    }
}

/// A light's choices, resolved against the book.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedChoices {
    /// The side of each change a person chose, by event index.
    pub sides: Vec<(usize, Side, LightChoiceRow)>,
    /// The group a person chose: its family and its sessions.
    pub group: Option<GroupPick>,
    /// Choices that find nothing to apply to, with why.
    pub orphaned: Vec<(LightChoiceRow, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupPick {
    pub family: usize,
    /// Index into the family's groups.
    pub group: usize,
    pub row: LightChoiceRow,
}

impl ResolvedChoices {
    pub fn side_of(&self, event: usize) -> Option<Side> {
        self.sides
            .iter()
            .find(|(index, _, _)| *index == event)
            .map(|(_, side, _)| *side)
    }

    pub fn is_empty(&self) -> bool {
        self.sides.is_empty() && self.group.is_none() && self.orphaned.is_empty()
    }
}

impl Book {
    /// The measured session a choice names: by its flat, else by its
    /// session key.
    pub fn session_named(&self, frame: Option<&str>, session_key: Option<&str>) -> Option<usize> {
        frame
            .and_then(|uuid| self.session_of_frame(uuid))
            .or_else(|| {
                let key = session_key?;
                self.sessions.iter().position(|s| s.row.session_key == key)
            })
    }

    /// The group of `family` that holds `session`, by index.
    pub fn group_of(&self, family: usize, session: usize) -> Option<usize> {
        self.families
            .get(family)?
            .groups
            .iter()
            .position(|group| group.contains(&session))
    }

    /// The choices recorded for a light read from `file_name`, of `filter`,
    /// shot at `at` on `rig`, resolved against this book.
    pub fn light_choices(
        &self,
        file_name: &str,
        filter: Option<&str>,
        rig: &str,
        at: Option<i64>,
    ) -> ResolvedChoices {
        let mut out = ResolvedChoices::default();
        let Some(entries) = self.light_choices.get(file_name) else {
            return out;
        };
        let canonical = filter.map(super::model::canonical_filter);
        // Two lights of one file name: the one of the light's filter, then
        // the one nearest its time.
        let same_filter = |entry: &&LightChoiceEntry| match (&canonical, &entry.filter) {
            (Some(light), Some(row)) => *light == super::model::canonical_filter(row),
            _ => true,
        };
        let candidates: Vec<&LightChoiceEntry> = entries.iter().filter(same_filter).collect();
        let Some(light_id) = candidates
            .iter()
            .min_by_key(|entry| match (at, entry.at) {
                (Some(a), Some(b)) => (a - b).abs(),
                _ => i64::MAX,
            })
            .map(|entry| entry.light_id)
        else {
            return out;
        };
        for entry in candidates.iter().filter(|e| e.light_id == light_id) {
            let row = &entry.row;
            let Some(choice) = row.choice else { continue };
            match choice {
                LightChoice::Group => match self.resolve_group(row) {
                    Ok(pick) => out.group = Some(pick),
                    Err(reason) => out.orphaned.push((row.clone(), reason)),
                },
                LightChoice::Before | LightChoice::After => {
                    match self.resolve_change(row, rig, filter) {
                        Ok(event) => {
                            out.sides
                                .push((event, choice.side().expect("a side"), row.clone()))
                        }
                        Err(reason) => out.orphaned.push((row.clone(), reason)),
                    }
                }
            }
        }
        out
    }

    /// The group a light's group choice names.
    fn resolve_group(&self, row: &LightChoiceRow) -> Result<GroupPick, String> {
        let session = self
            .session_named(row.group_frame.as_deref(), row.group_session_key.as_deref())
            .ok_or_else(|| "the flats of the group it named are gone".to_string())?;
        let family = *self
            .family_of
            .get(session)
            .filter(|f| **f != usize::MAX)
            .ok_or_else(|| "the flats of the group it named are in no family".to_string())?;
        let group = self
            .group_of(family, session)
            .ok_or_else(|| "the group it named is gone".to_string())?;
        Ok(GroupPick {
            family,
            group,
            row: row.clone(),
        })
    }

    /// The change a light's side choice names: by its key, else the one
    /// change of the rig whose window overlaps the window it was made for
    /// (new flats narrow a window, and that renames the change).
    fn resolve_change(
        &self,
        row: &LightChoiceRow,
        rig: &str,
        filter: Option<&str>,
    ) -> Result<usize, String> {
        let of_rig = |event: &&super::model::Event| event.rig_uuid == rig;
        let found = self
            .events
            .iter()
            .position(|event| event.key == row.change_key && of_rig(&event));
        let found = match found {
            Some(index) => Some(index),
            None => {
                let (Some(start), Some(end)) = (row.change_start, row.change_end) else {
                    return Err("the change it was made for is gone".into());
                };
                let overlapping: Vec<usize> = self
                    .events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| of_rig(event) && event.start < end && event.end > start)
                    .map(|(index, _)| index)
                    .collect();
                match overlapping.as_slice() {
                    [one] => Some(*one),
                    [] => None,
                    _ => {
                        return Err(
                            "the change it was made for is now two changes; choose again".into(),
                        );
                    }
                }
            }
        };
        let index = found.ok_or_else(|| "the change it was made for is gone".to_string())?;
        if !self.events[index].applies_to(filter) {
            return Err("the change no longer reaches this light's filter".into());
        }
        Ok(index)
    }
}
