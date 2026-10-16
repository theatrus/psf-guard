//! When periodic backups run and how many they keep.
//!
//! The settings live in the registry under `backups`. A run is due at the
//! chosen time of day, every day or on one weekday, in the server's local
//! time. A server that was off at that moment runs once when it comes back;
//! it does not run every slot it missed.

use super::BackupScope;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    #[default]
    Daily,
    Weekly,
}

/// Which of the job's own backups a run keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Retention {
    /// The newest `count`.
    Count { count: u32 },
    /// Those younger than `days`; the newest is always kept.
    Age { days: u32 },
}

impl Default for Retention {
    fn default() -> Self {
        Retention::Count {
            count: DEFAULT_KEEP,
        }
    }
}

/// A week of daily backups.
pub const DEFAULT_KEEP: u32 = 7;
pub const MAX_KEEP: u32 = 1000;
pub const MAX_AGE_DAYS: u32 = 3650;

/// Midday: imaging runs at night, so the catalogs are quiet by day.
pub const DEFAULT_TIME: &str = "13:00";

/// The periodic backup a person set up in Settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSettings {
    /// Off unless a person turns it on.
    pub enabled: bool,
    pub frequency: Frequency,
    /// For a weekly schedule: 0 is Monday, 6 is Sunday.
    pub weekday: u8,
    /// Local time of day, `HH:MM`.
    pub time: String,
    /// The folder the archives go to. Required to turn the schedule on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    pub scope: BackupScope,
    /// Keep peer keys and other credentials in the archive.
    pub include_secrets: bool,
    pub retention: Retention,
    /// Unix seconds when the schedule was turned on or last changed. Slots
    /// before it never count as missed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// This install's id, minted when the schedule is first saved. Archives
    /// carry it, and pruning removes only archives with the same id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency: Frequency::Daily,
            weekday: 6,
            time: DEFAULT_TIME.to_string(),
            destination: None,
            scope: BackupScope::Full,
            include_secrets: false,
            retention: Retention::default(),
            since: None,
            install_id: None,
        }
    }
}

impl BackupSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        parse_time(&self.time)?;
        if self.weekday > 6 {
            anyhow::bail!("weekday must be 0 (Monday) to 6 (Sunday)");
        }
        match self.retention {
            Retention::Count { count } if count == 0 || count > MAX_KEEP => {
                anyhow::bail!("keep between 1 and {MAX_KEEP} backups")
            }
            Retention::Age { days } if days == 0 || days > MAX_AGE_DAYS => {
                anyhow::bail!("keep backups between 1 and {MAX_AGE_DAYS} days")
            }
            _ => {}
        }
        if self.enabled && self.destination_path().is_none() {
            anyhow::bail!("choose a folder for the backups before turning them on");
        }
        if let Some(destination) = self.destination_path()
            && !std::path::Path::new(destination).is_absolute()
        {
            anyhow::bail!("the backup folder must be an absolute path");
        }
        Ok(())
    }

    /// The destination, trimmed, when one is set.
    pub fn destination_path(&self) -> Option<&str> {
        self.destination
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
    }

    /// Whether `other` runs at different moments, so `since` moves.
    pub fn timing_differs(&self, other: &Self) -> bool {
        self.enabled != other.enabled
            || self.frequency != other.frequency
            || self.time != other.time
            || (self.frequency == Frequency::Weekly && self.weekday != other.weekday)
            || self.destination_path() != other.destination_path()
    }
}

/// `HH:MM`, 24-hour.
pub fn parse_time(text: &str) -> anyhow::Result<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M")
        .map_err(|_| anyhow::anyhow!("time of day must be HH:MM, such as 13:00"))
}

/// The moment `time` falls on `date` in `zone`. A time a clock change skips
/// runs when the clock reaches it, an hour on; a time that happens twice
/// runs the first time.
fn slot_on<Tz: TimeZone>(date: NaiveDate, time: NaiveTime, zone: &Tz) -> DateTime<Utc> {
    let local = date.and_time(time);
    match zone.from_local_datetime(&local).earliest() {
        Some(moment) => moment.with_timezone(&Utc),
        None => zone
            .from_local_datetime(&(local + Duration::hours(1)))
            .earliest()
            .map(|moment| moment.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&local)),
    }
}

fn runs_on(settings: &BackupSettings, date: NaiveDate) -> bool {
    match settings.frequency {
        Frequency::Daily => true,
        Frequency::Weekly => date.weekday().num_days_from_monday() == u32::from(settings.weekday),
    }
}

/// The last slot at or before `at`.
pub fn latest_slot<Tz: TimeZone>(
    settings: &BackupSettings,
    at: DateTime<Utc>,
    zone: &Tz,
) -> Option<DateTime<Utc>> {
    let time = parse_time(&settings.time).ok()?;
    let today = at.with_timezone(zone).date_naive();
    (0..=8)
        .filter_map(|back| today.checked_sub_signed(Duration::days(back)))
        .filter(|date| runs_on(settings, *date))
        .map(|date| slot_on(date, time, zone))
        .find(|slot| *slot <= at)
}

/// The first slot after `at`.
pub fn next_slot<Tz: TimeZone>(
    settings: &BackupSettings,
    at: DateTime<Utc>,
    zone: &Tz,
) -> Option<DateTime<Utc>> {
    let time = parse_time(&settings.time).ok()?;
    let today = at.with_timezone(zone).date_naive();
    (0..=8)
        .filter_map(|ahead| today.checked_add_signed(Duration::days(ahead)))
        .filter(|date| runs_on(settings, *date))
        .map(|date| slot_on(date, time, zone))
        .find(|slot| *slot > at)
}

/// When the next scheduled run is due. A moment at or before `now` means it
/// is due now: the slot it stands for passed after `last_run` (the newest
/// scheduled backup, or the last attempt that should not be repeated) and
/// after the schedule was set. `None` when the schedule is off.
pub fn next_run<Tz: TimeZone>(
    settings: &BackupSettings,
    last_run: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Option<DateTime<Utc>> {
    if !settings.enabled || settings.destination_path().is_none() {
        return None;
    }
    let since = settings
        .since
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single());
    let anchor = match (last_run, since) {
        (Some(last), Some(since)) => Some(last.max(since)),
        (last, since) => last.or(since),
    };
    if let (Some(anchor), Some(missed)) = (anchor, latest_slot(settings, now, zone))
        && missed > anchor
    {
        return Some(missed);
    }
    next_slot(settings, anchor.map_or(now, |anchor| anchor.max(now)), zone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn daily(time: &str) -> BackupSettings {
        BackupSettings {
            enabled: true,
            time: time.into(),
            destination: Some("/backups".into()),
            ..Default::default()
        }
    }

    #[test]
    fn defaults_are_off_full_midday_and_a_week_kept() {
        let settings = BackupSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.scope, BackupScope::Full);
        assert_eq!(settings.time, "13:00");
        assert_eq!(settings.retention, Retention::Count { count: 7 });
        assert!(!settings.include_secrets);
        settings.validate().unwrap();
    }

    #[test]
    fn a_schedule_needs_an_absolute_folder_and_sane_numbers() {
        let mut settings = daily("13:00");
        settings.destination = None;
        assert!(settings.validate().is_err());
        settings.destination = Some("relative/backups".into());
        assert!(settings.validate().is_err());
        let mut settings = daily("25:00");
        assert!(settings.validate().is_err());
        settings.time = "13:00".into();
        settings.retention = Retention::Count { count: 0 };
        assert!(settings.validate().is_err());
        settings.retention = Retention::Age { days: 30 };
        settings.validate().unwrap();
    }

    #[test]
    fn the_next_daily_run_is_today_or_tomorrow() {
        let zone = FixedOffset::west_opt(7 * 3600).unwrap(); // UTC−7
        let mut settings = daily("13:00");
        settings.since = Some(at("2026-10-16T08:00:00-07:00").timestamp());
        let now = at("2026-10-16T09:00:00-07:00");
        assert_eq!(
            next_run(&settings, None, now, &zone),
            Some(at("2026-10-16T13:00:00-07:00"))
        );
        // After today's run, tomorrow's.
        let last = at("2026-10-16T13:00:05-07:00");
        let later = at("2026-10-16T15:00:00-07:00");
        assert_eq!(
            next_run(&settings, Some(last), later, &zone),
            Some(at("2026-10-17T13:00:00-07:00"))
        );
    }

    #[test]
    fn a_slot_missed_while_the_server_was_off_runs_once_on_return() {
        let zone = FixedOffset::east_opt(0).unwrap();
        let mut settings = daily("13:00");
        settings.since = Some(at("2026-10-01T00:00:00Z").timestamp());
        let last = at("2026-10-14T13:00:02Z");
        // Off on the 15th and the 16th; back at 15:00 on the 16th.
        let now = at("2026-10-16T15:00:00Z");
        let due = next_run(&settings, Some(last), now, &zone).unwrap();
        assert!(due <= now, "the missed run is due now");
        assert_eq!(due, at("2026-10-16T13:00:00Z"));
        // Once it has run, the next is tomorrow, not the 15th's slot.
        let after = at("2026-10-16T15:01:00Z");
        assert_eq!(
            next_run(&settings, Some(after), after, &zone),
            Some(at("2026-10-17T13:00:00Z"))
        );
    }

    #[test]
    fn turning_it_on_after_todays_slot_waits_for_tomorrow() {
        let zone = FixedOffset::east_opt(0).unwrap();
        let mut settings = daily("13:00");
        let now = at("2026-10-16T15:00:00Z");
        settings.since = Some(now.timestamp());
        assert_eq!(
            next_run(&settings, None, now, &zone),
            Some(at("2026-10-17T13:00:00Z"))
        );
    }

    #[test]
    fn a_weekly_run_lands_on_its_weekday() {
        let zone = FixedOffset::east_opt(2 * 3600).unwrap();
        let mut settings = daily("03:30");
        settings.frequency = Frequency::Weekly;
        settings.weekday = 6; // Sunday
        let now = at("2026-10-16T12:00:00+02:00"); // a Friday
        settings.since = Some(now.timestamp());
        assert_eq!(
            next_run(&settings, None, now, &zone),
            Some(at("2026-10-18T03:30:00+02:00"))
        );
        // Wednesday instead.
        settings.weekday = 2;
        assert_eq!(
            next_run(&settings, None, now, &zone),
            Some(at("2026-10-21T03:30:00+02:00"))
        );
    }

    #[test]
    fn off_or_without_a_folder_never_runs() {
        let zone = FixedOffset::east_opt(0).unwrap();
        let now = at("2026-10-16T15:00:00Z");
        let mut settings = daily("13:00");
        settings.enabled = false;
        assert_eq!(next_run(&settings, None, now, &zone), None);
        let mut settings = daily("13:00");
        settings.destination = Some("  ".into());
        assert_eq!(next_run(&settings, None, now, &zone), None);
    }

    #[test]
    fn a_change_of_time_or_folder_moves_the_anchor_but_retention_does_not() {
        let base = daily("13:00");
        let mut other = base.clone();
        other.retention = Retention::Count { count: 3 };
        assert!(!base.timing_differs(&other));
        other.time = "14:00".into();
        assert!(base.timing_differs(&other));
    }

    #[test]
    fn the_settings_round_trip_through_json_and_tolerate_gaps() {
        let parsed: BackupSettings =
            serde_json::from_str(r#"{"enabled":true,"retention":{"kind":"age","days":30}}"#)
                .unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.retention, Retention::Age { days: 30 });
        assert_eq!(parsed.time, DEFAULT_TIME);
        let text = serde_json::to_string(&parsed).unwrap();
        assert_eq!(
            serde_json::from_str::<BackupSettings>(&text).unwrap(),
            parsed
        );
    }
}
