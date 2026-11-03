//! A Director rig's overrides for one night: what to leave out, where only
//! one plan, target or exposure plan may run, the night's plan order, and a
//! hold. They narrow the program the rig pulls and Director's preview of
//! the night, and nothing else: no Target Scheduler database, N.I.N.A.
//! setting or other rig is written. A night's overrides lapse when the
//! night ends.
//!
//! A rig's writes share one counter, so every write of a night carries a
//! revision no earlier write of it had. A change is checked against the
//! night's own revision, so editing one night never refuses an edit of
//! another.

use super::*;
use psf_guard_director_core::{night::NightDate, windows::Interval};

/// Most overrides one night may carry.
pub const MAX_ENTRIES: usize = 32;
/// Most plans a night's order may list.
pub const MAX_ORDERED_PLANS: usize = 256;
/// Most nights one read returns.
pub const MAX_NIGHTS_PER_READ: usize = 400;

/// What an override applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Subject {
    /// Every exposure plan of a plan.
    Plan { plan_id: Uuid },
    /// Every exposure plan of one target in the rig's database.
    Target { target_guid: Uuid },
    /// One exposure plan in the rig's database.
    ExposurePlan { exposureplan_guid: Uuid },
}

/// One override of a night.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NightOverride {
    /// The subject gets no time inside `window`, or none all night.
    Exclude {
        subject: Subject,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<Interval>,
    },
    /// Inside `window` nothing but the subject may run. If the sky or the
    /// Moon rules the subject out there, the window stays idle.
    Pin { subject: Subject, window: Interval },
    /// The night's plan order, first first, in place of the rig's. Plans it
    /// does not list follow, by name.
    Order { plans: Vec<Uuid> },
    /// The rig takes no work from `from_ms` to the night's end, or all night.
    Hold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_ms: Option<u64>,
    },
}

/// Why an override list was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooMany,
    /// A window or hold outside the night, or a window that ends before it
    /// starts.
    OutsideNight,
    /// Two pins share time: each would leave the other's window idle.
    PinsOverlap,
    /// More than one order, or more than one hold.
    Repeated,
    /// An order with no plans, too many, or a plan listed twice.
    BadOrder,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooMany => "A night can have at most 32 overrides.",
            Self::OutsideNight => {
                "Each window and hold must fall inside the night, from noon to noon, and end after it starts."
            }
            Self::PinsOverlap => "Two pins overlap. Move one.",
            Self::Repeated => "A night can have at most one plan order and one hold.",
            Self::BadOrder => "A plan order must list each plan once, and at most 256 plans.",
        })
    }
}

/// Check a night's override list against the night's noon-to-noon span.
pub fn check(entries: &[NightOverride], night: Interval) -> Result<(), Refusal> {
    if entries.len() > MAX_ENTRIES {
        return Err(Refusal::TooMany);
    }
    let inside = |window: &Interval| {
        window.start_ms < window.end_ms
            && window.start_ms >= night.start_ms
            && window.end_ms <= night.end_ms
    };
    let mut pins: Vec<Interval> = Vec::new();
    let (mut orders, mut holds) = (0, 0);
    for entry in entries {
        match entry {
            NightOverride::Exclude { window, .. } => {
                if window.as_ref().is_some_and(|w| !inside(w)) {
                    return Err(Refusal::OutsideNight);
                }
            }
            NightOverride::Pin { window, .. } => {
                if !inside(window) {
                    return Err(Refusal::OutsideNight);
                }
                pins.push(*window);
            }
            NightOverride::Order { plans } => {
                orders += 1;
                let distinct: std::collections::BTreeSet<_> = plans.iter().collect();
                if plans.is_empty()
                    || plans.len() > MAX_ORDERED_PLANS
                    || distinct.len() != plans.len()
                {
                    return Err(Refusal::BadOrder);
                }
            }
            NightOverride::Hold { from_ms } => {
                holds += 1;
                if from_ms.is_some_and(|from| from < night.start_ms || from >= night.end_ms) {
                    return Err(Refusal::OutsideNight);
                }
            }
        }
    }
    if orders > 1 || holds > 1 {
        return Err(Refusal::Repeated);
    }
    pins.sort_by_key(|w| w.start_ms);
    if pins
        .windows(2)
        .any(|pair| pair[1].start_ms < pair[0].end_ms)
    {
        return Err(Refusal::PinsOverlap);
    }
    Ok(())
}

/// One rig's overrides for one night.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NightOverrides {
    pub date: NightDate,
    pub entries: Vec<NightOverride>,
    /// Send this back with a change of the night.
    pub revision: u64,
    pub updated_at_ms: u64,
}

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS night_override_rig(
            rig_id TEXT PRIMARY KEY NOT NULL REFERENCES rig(id),
            revision INTEGER NOT NULL CHECK(revision>0));
         CREATE TABLE IF NOT EXISTS night_override(
            rig_id TEXT NOT NULL REFERENCES night_override_rig(rig_id),
            night_date TEXT NOT NULL,
            entries TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(rig_id,night_date));",
    )?;
    Ok(())
}

pub(crate) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT rig_id,revision FROM night_override_rig LIMIT 0",
        "SELECT rig_id,night_date,entries,revision,updated_at_ms FROM night_override LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    Ok(())
}

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

fn to_sql(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::InvalidInput)
}

impl MetaStore {
    /// The rig's overrides for the nights `from` to `to`, both included, by
    /// date. Nights without overrides are left out.
    pub fn night_overrides(
        &self,
        rig: Uuid,
        from: NightDate,
        to: NightDate,
    ) -> Result<Vec<NightOverrides>, Error> {
        valid_id(rig)?;
        if to < from || from.days_until(to) >= MAX_NIGHTS_PER_READ as i64 {
            return Err(Error::InvalidInput);
        }
        let mut statement = self.connection.prepare(
            "SELECT night_date,entries,revision,updated_at_ms FROM night_override
             WHERE rig_id=?1 AND night_date>=?2 AND night_date<=?3 ORDER BY night_date",
        )?;
        let rows = statement.query_map(
            params![rig.to_string(), from.to_string(), to.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (date, entries, revision, updated) = row?;
            Ok(NightOverrides {
                date: date.parse().map_err(|_| Error::CorruptDatabase)?,
                entries: serde_json::from_str(&entries).map_err(|_| Error::CorruptDatabase)?,
                revision: u64::try_from(revision).map_err(|_| Error::CorruptDatabase)?,
                updated_at_ms: u64::try_from(updated).map_err(|_| Error::CorruptDatabase)?,
            })
        })
        .collect()
    }

    /// One night's overrides, or `None` when it has none.
    pub fn night_override(
        &self,
        rig: Uuid,
        date: NightDate,
    ) -> Result<Option<NightOverrides>, Error> {
        Ok(self.night_overrides(rig, date, date)?.pop())
    }

    /// Replace a night's overrides. `expected_revision` is the night's
    /// revision as read, 0 when it had none; any other refuses the write
    /// with `Conflict`. An empty list returns the night to the rig's own
    /// program, and gives `None`. The caller checks the list against the
    /// night with [`check`], which needs the rig's longitude.
    pub fn save_night_overrides(
        &mut self,
        rig: Uuid,
        date: NightDate,
        entries: &[NightOverride],
        expected_revision: u64,
        now_ms: u64,
    ) -> Result<Option<NightOverrides>, Error> {
        if entries.len() > MAX_ENTRIES || now_ms > MAX_TIME_MS {
            return Err(Error::InvalidInput);
        }
        let payload = serde_json::to_string(entries).map_err(|_| Error::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Rig, rig)?.is_none() {
            return Err(Error::NotFound);
        }
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM night_override WHERE rig_id=?1 AND night_date=?2",
                params![rig.to_string(), date.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let current = match current {
            None => 0,
            Some(value) => u64::try_from(value).map_err(|_| Error::CorruptDatabase)?,
        };
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        let counter: Option<i64> = tx
            .query_row(
                "SELECT revision FROM night_override_rig WHERE rig_id=?1",
                [rig.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let next = counter
            .map_or(Ok(0), u64::try_from)
            .map_err(|_| Error::CorruptDatabase)?
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        tx.execute(
            "INSERT INTO night_override_rig(rig_id,revision) VALUES(?1,?2)
             ON CONFLICT(rig_id) DO UPDATE SET revision=excluded.revision",
            params![rig.to_string(), to_sql(next)?],
        )?;
        tx.execute(
            "DELETE FROM night_override WHERE rig_id=?1 AND night_date=?2",
            params![rig.to_string(), date.to_string()],
        )?;
        let saved = if entries.is_empty() {
            None
        } else {
            tx.execute(
                "INSERT INTO night_override(rig_id,night_date,entries,revision,updated_at_ms)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    rig.to_string(),
                    date.to_string(),
                    payload,
                    to_sql(next)?,
                    to_sql(now_ms)?
                ],
            )?;
            Some(NightOverrides {
                date,
                entries: entries.to_vec(),
                revision: next,
                updated_at_ms: now_ms,
            })
        };
        tx.commit()?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn night() -> Interval {
        Interval {
            start_ms: 1_000_000,
            end_ms: 1_000_000 + 86_400_000,
        }
    }

    fn within(start: u64, end: u64) -> Interval {
        Interval {
            start_ms: night().start_ms + start,
            end_ms: night().start_ms + end,
        }
    }

    #[test]
    fn a_list_is_refused_for_what_it_would_get_wrong() {
        let subject = Subject::Plan {
            plan_id: Uuid::from_u128(1),
        };
        let pin = |start, end| NightOverride::Pin {
            subject,
            window: within(start, end),
        };
        assert_eq!(check(&[pin(10, 20), pin(30, 40)], night()), Ok(()));
        assert_eq!(
            check(&[pin(10, 30), pin(20, 40)], night()),
            Err(Refusal::PinsOverlap)
        );
        assert_eq!(
            check(&[pin(10, 86_400_001)], night()),
            Err(Refusal::OutsideNight)
        );
        assert_eq!(check(&[pin(20, 20)], night()), Err(Refusal::OutsideNight));
        let hold = |from_ms| NightOverride::Hold { from_ms };
        assert_eq!(check(&[hold(None)], night()), Ok(()));
        assert_eq!(
            check(&[hold(Some(night().end_ms))], night()),
            Err(Refusal::OutsideNight)
        );
        assert_eq!(
            check(&[hold(None), hold(None)], night()),
            Err(Refusal::Repeated)
        );
        let order = |plans: Vec<u128>| NightOverride::Order {
            plans: plans.into_iter().map(Uuid::from_u128).collect(),
        };
        assert_eq!(check(&[order(vec![1, 2])], night()), Ok(()));
        assert_eq!(check(&[order(vec![1, 1])], night()), Err(Refusal::BadOrder));
        assert_eq!(check(&[order(vec![])], night()), Err(Refusal::BadOrder));
        assert_eq!(
            check(&vec![hold(None); MAX_ENTRIES + 1], night()),
            Err(Refusal::TooMany)
        );
    }

    fn date(text: &str) -> NightDate {
        text.parse().unwrap()
    }

    fn hold() -> Vec<NightOverride> {
        vec![NightOverride::Hold { from_ms: None }]
    }

    #[test]
    fn a_nights_overrides_round_trip_and_a_stale_or_reused_revision_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let rig = Uuid::new_v4();
        store.create_rig(rig, "North").unwrap();
        let tonight = date("2026-11-02");
        assert_eq!(store.night_override(rig, tonight).unwrap(), None);
        let saved = store
            .save_night_overrides(rig, tonight, &hold(), 0, 10)
            .unwrap()
            .unwrap();
        assert_eq!((saved.revision, saved.updated_at_ms), (1, 10));
        assert_eq!(
            store.night_override(rig, tonight).unwrap(),
            Some(saved.clone())
        );
        // A writer that read the night before the first save is refused.
        assert!(matches!(
            store.save_night_overrides(rig, tonight, &[], 0, 11),
            Err(Error::Conflict)
        ));
        // Another night's revisions are its own: its first write expects 0.
        let tomorrow = date("2026-11-03");
        let other = store
            .save_night_overrides(rig, tomorrow, &hold(), 0, 12)
            .unwrap()
            .unwrap();
        assert_eq!(other.revision, 2);
        // Clearing a night, then saving it again, never reuses a revision,
        // so a writer holding the first one is still refused.
        assert_eq!(
            store
                .save_night_overrides(rig, tonight, &[], 1, 13)
                .unwrap(),
            None
        );
        assert_eq!(store.night_override(rig, tonight).unwrap(), None);
        let again = store
            .save_night_overrides(rig, tonight, &hold(), 0, 14)
            .unwrap()
            .unwrap();
        assert_eq!(again.revision, 4);
        assert!(matches!(
            store.save_night_overrides(rig, tonight, &[], 1, 15),
            Err(Error::Conflict)
        ));
        let range = store
            .night_overrides(rig, date("2026-11-01"), date("2026-11-30"))
            .unwrap();
        assert_eq!(
            range.iter().map(|n| n.date).collect::<Vec<_>>(),
            [tonight, tomorrow]
        );
        assert!(matches!(
            store.save_night_overrides(Uuid::new_v4(), tonight, &hold(), 0, 16),
            Err(Error::NotFound)
        ));
        assert!(matches!(
            store.night_overrides(rig, tomorrow, tonight),
            Err(Error::InvalidInput)
        ));
    }

    #[test]
    fn a_schema_34_store_gains_the_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        drop(MetaStore::create(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE night_override; DROP TABLE night_override_rig; PRAGMA user_version=34;",
        )
        .unwrap();
        drop(conn);
        let mut store = MetaStore::open(&path).unwrap();
        let rig = Uuid::new_v4();
        store.create_rig(rig, "North").unwrap();
        assert!(store
            .save_night_overrides(rig, date("2026-11-02"), &hold(), 0, 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn overrides_read_back_as_written_in_their_wire_form() {
        let entries = vec![
            NightOverride::Exclude {
                subject: Subject::Target {
                    target_guid: Uuid::from_u128(7),
                },
                window: None,
            },
            NightOverride::Hold {
                from_ms: Some(1_500_000),
            },
        ];
        let json = serde_json::to_value(&entries).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"kind": "exclude", "subject": {"kind": "target", "target_guid": Uuid::from_u128(7)}},
                {"kind": "hold", "from_ms": 1_500_000}
            ])
        );
        let back: Vec<NightOverride> = serde_json::from_value(json).unwrap();
        assert_eq!(back, entries);
        assert!(serde_json::from_value::<NightOverride>(
            serde_json::json!({"kind": "hold", "from": 1})
        )
        .is_err());
    }
}
