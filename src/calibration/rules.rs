//! The calibration checks a person can tune, for the whole server and for
//! one rig.
//!
//! Matching, the light checks and the frame grades each read their limits
//! from a [`Rules`]. A value comes from the rig's own settings when it has
//! one, then the server's, then the built-in default. The settings live in
//! the database registry ([`crate::db_registry::CalibrationSettings`]) as
//! maps of key to value: a key a newer build wrote survives a save here,
//! and a value this build cannot read falls through to the next layer.
//!
//! [`fields`] lists every setting with its range and default. The settings
//! API sends it to the panel, validates edits against it, and the defaults
//! come from the code that used to hard-code them, so they cannot drift.
//! The table, its validation and its resolution are Seiza's
//! (`seiza_calibration::library::rules`), shared with the desktop apps; this
//! module keeps the settings in force here.
//!
//! A rig is the calibration library's rig ([`super::rig_signature`]): its
//! telescope, camera, frame size and binning. The UUID is a hash of those,
//! so one rig has the same key in every catalog, and a light's rig is read
//! off its own header.

use crate::commands::import::headers::FrameMeta;
pub use seiza_calibration::library::rules::*;
use std::sync::{Arc, LazyLock, RwLock};

/// Who set a value, in words.
pub trait SetBy {
    /// "set globally" or "set for SpaceCat61 · ASI2600MM Pro", or `None`
    /// when the value is the default.
    fn who_set(&self, key: impl Into<SettingKey>) -> Option<String>;
    /// `", set …"` after a limit, or nothing for a default.
    fn who_set_suffix(&self, key: impl Into<SettingKey>) -> String {
        self.who_set(key)
            .map(|by| format!(", {by}"))
            .unwrap_or_default()
    }
}

impl SetBy for Rules {
    fn who_set(&self, key: impl Into<SettingKey>) -> Option<String> {
        match self.set_by(key)? {
            RuleSource::Default => None,
            RuleSource::Global => Some("set globally".into()),
            RuleSource::Rig => Some(format!(
                "set for {}",
                self.rig
                    .as_ref()
                    .map_or("this rig", |rig| rig.name.as_str())
            )),
        }
    }
}

/// The rig a light was shot with, by the key the library files rigs
/// under.
pub fn light_rig_uuid(light: &FrameMeta) -> String {
    super::stable_uuid(&super::rig_signature(None, light))
}

static BOOK: LazyLock<RwLock<Arc<RuleBook>>> =
    LazyLock::new(|| RwLock::new(Arc::new(RuleBook::default())));

#[cfg(test)]
thread_local! {
    /// Settings one test put in force for itself, seen only on its own
    /// thread, so the tests running beside it keep the defaults.
    static TEST_BOOK: std::cell::RefCell<Option<Arc<RuleBook>>> =
        const { std::cell::RefCell::new(None) };
}

/// Put settings in force for the calling test's thread alone, or `None`
/// to go back to the process's.
#[cfg(test)]
pub(crate) fn set_test_rule_book(book: Option<RuleBook>) {
    TEST_BOOK.with(|slot| *slot.borrow_mut() = book.map(Arc::new));
}

/// The settings in force.
pub fn rule_book() -> Arc<RuleBook> {
    #[cfg(test)]
    if let Some(book) = TEST_BOOK.with(|slot| slot.borrow().clone()) {
        return book;
    }
    Arc::clone(&BOOK.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Put new settings in force for every match, check and grade after this.
pub fn set_rule_book(book: RuleBook) {
    *BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(book);
}

/// Change the server's layer and keep every rig's.
pub(crate) fn edit_global(edit: impl FnOnce(&mut RuleValues)) {
    let mut guard = BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut global = guard.global_values().clone();
    edit(&mut global);
    *guard = Arc::new(guard.with_global(global));
}

/// The built-in defaults, whatever the settings: for questions about a
/// frame itself rather than about what it calibrates.
pub fn defaults() -> &'static Rules {
    static DEFAULTS: LazyLock<Rules> = LazyLock::new(Rules::default);
    &DEFAULTS
}

/// The rules a light is matched and checked with.
pub fn rules_for_light(light: &FrameMeta) -> Arc<Rules> {
    rule_book().for_rig(&light_rig_uuid(light))
}

/// The rules a calibration frame of this rig is graded with.
pub fn rules_for_rig(rig_uuid: &str) -> Arc<Rules> {
    rule_book().for_rig(rig_uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn values(pairs: &[(&str, Value)]) -> RuleValues {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn a_value_says_who_set_it() {
        assert_eq!(Rules::default().who_set(SettingKey::DarkReachDays), None);
        let global = values(&[
            ("dark_reach_days", json!(90)),
            ("thin_master_frames", json!(4)),
        ]);
        let rig = RigLayer {
            name: Some("SpaceCat61 · ASI2600MM Pro".into()),
            values: values(&[("dark_reach_days", json!(400))]),
        };
        let book = RuleBook::new(global, BTreeMap::from([("rig-a".to_string(), rig)]));
        let rig_rules = book.for_rig("rig-a");
        assert_eq!(
            rig_rules.who_set(SettingKey::DarkReachDays).as_deref(),
            Some("set for SpaceCat61 · ASI2600MM Pro")
        );
        assert_eq!(
            rig_rules.who_set(SettingKey::ThinMasterFrames).as_deref(),
            Some("set globally")
        );
        assert_eq!(
            rig_rules.who_set_suffix(SettingKey::ThinMasterFrames),
            ", set globally"
        );
        assert_eq!(rig_rules.who_set(SettingKey::CompleteDarkFrames), None);
    }

    #[test]
    fn a_light_takes_its_own_rigs_rules() {
        let rig = RigLayer {
            name: None,
            values: values(&[("dark_reach_days", json!(400))]),
        };
        let light = FrameMeta::default();
        set_test_rule_book(Some(RuleBook::new(
            RuleValues::new(),
            BTreeMap::from([(light_rig_uuid(&light), rig)]),
        )));
        assert_eq!(rules_for_light(&light).dark_reach_days, 400.0);
        assert_eq!(rules_for_rig("another").dark_reach_days, 183.0);
        set_test_rule_book(None);
    }
}
