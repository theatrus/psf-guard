//! Night availability: marks that a night is clear, out, or clear only in
//! some hours, for a site (the sky) or for one rig (the equipment). Planning
//! input only. Nothing here reaches a program, the plugin, Target Scheduler
//! or N.I.N.A., and a mark never allows or blocks acquisition.
//!
//! Each scope's marks share one compare-and-set revision, so writing a range
//! of nights is one check and one transaction.

use super::*;
use psf_guard_director_core::{night::NightDate, windows::Interval};
use std::str::FromStr;

/// Most windows one night's mark may carry.
pub const MAX_MARK_WINDOWS: usize = 8;
/// Most nights one write may mark or clear: two months.
pub const MAX_NIGHTS_PER_WRITE: usize = 62;
/// Most nights one read returns.
pub const MAX_NIGHTS_PER_READ: usize = 400;
pub const MAX_NOTE_CHARS: usize = 200;

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

/// What a mark covers: a site's sky, or one rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkScope {
    /// Every rig whose planning site this is.
    Site,
    /// One rig, whatever its site's sky does.
    Rig,
}

impl MarkScope {
    fn as_column(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Rig => "rig",
        }
    }

    fn kind(self) -> Kind {
        match self {
            Self::Site => Kind::Site,
            Self::Rig => Kind::Rig,
        }
    }
}

impl fmt::Display for MarkScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_column())
    }
}

impl FromStr for MarkScope {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "site" => Ok(Self::Site),
            "rig" => Ok(Self::Rig),
            _ => Err(Error::InvalidInput),
        }
    }
}

/// A night's availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Clear all night.
    Available,
    /// Nothing that night: clouds, rain, the rig down, nobody home.
    Unavailable,
    /// Clear only inside the mark's windows.
    Limited,
}

impl Availability {
    fn as_column(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Limited => "limited",
        }
    }
}

impl fmt::Display for Availability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_column())
    }
}

impl FromStr for Availability {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "available" => Ok(Self::Available),
            "unavailable" => Ok(Self::Unavailable),
            "limited" => Ok(Self::Limited),
            _ => Err(Error::InvalidInput),
        }
    }
}

/// One night's mark at one scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NightMark {
    pub date: NightDate,
    pub state: Availability,
    /// For a `Limited` night, the clear hours as UTC instants inside the
    /// night's noon-to-noon span, sorted and apart. Empty otherwise.
    pub windows: Vec<Interval>,
    /// A short reason, such as "dome motor" or "high cirrus".
    pub note: String,
    /// The scope revision that wrote this mark.
    pub revision: u64,
    pub updated_at_ms: u64,
}

/// A night to mark: what a person enters, before the store stamps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkEdit {
    pub date: NightDate,
    pub state: Availability,
    #[serde(default)]
    pub windows: Vec<Interval>,
    #[serde(default)]
    pub note: String,
}

impl MarkEdit {
    /// Check the edit's shape and merge its windows. Whether the windows
    /// fall inside the night needs the scope's longitude, which the caller
    /// checks with [`NightDate::span`].
    pub fn normalized(&self) -> Result<Self, Error> {
        if self.note.chars().count() > MAX_NOTE_CHARS || self.note.chars().any(char::is_control) {
            return Err(Error::InvalidInput);
        }
        if self.windows.len() > MAX_MARK_WINDOWS
            || self
                .windows
                .iter()
                .any(|w| w.start_ms >= w.end_ms || w.end_ms > MAX_TIME_MS)
        {
            return Err(Error::InvalidInput);
        }
        let windows = merge(&self.windows);
        match (self.state, windows.is_empty()) {
            (Availability::Limited, true) => return Err(Error::InvalidInput),
            (Availability::Available | Availability::Unavailable, false) => {
                return Err(Error::InvalidInput);
            }
            _ => {}
        }
        Ok(Self {
            date: self.date,
            state: self.state,
            windows,
            note: self.note.trim().to_owned(),
        })
    }
}

/// A scope's marks over a range of nights.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NightMarks {
    pub scope: MarkScope,
    pub scope_id: Uuid,
    /// Send this back with a change: 0 before the first write.
    pub revision: u64,
    /// By date.
    pub marks: Vec<NightMark>,
}

fn merge(windows: &[Interval]) -> Vec<Interval> {
    let mut sorted = windows.to_vec();
    sorted.sort_by_key(|w| (w.start_ms, w.end_ms));
    let mut result: Vec<Interval> = Vec::with_capacity(sorted.len());
    for window in sorted {
        match result.last_mut() {
            Some(previous) if window.start_ms <= previous.end_ms => {
                previous.end_ms = previous.end_ms.max(window.end_ms);
            }
            _ => result.push(window),
        }
    }
    result
}

/// The only source today; forecasts will add their own beside it.
const MANUAL: &str = "manual";

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS night_availability_scope(
            scope TEXT NOT NULL CHECK(scope IN ('site','rig')),
            scope_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            PRIMARY KEY(scope,scope_id));
         CREATE TABLE IF NOT EXISTS night_availability(
            scope TEXT NOT NULL,
            scope_id TEXT NOT NULL,
            night_date TEXT NOT NULL,
            source TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('available','unavailable','limited')),
            note TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(scope,scope_id,night_date,source),
            FOREIGN KEY(scope,scope_id) REFERENCES night_availability_scope(scope,scope_id));
         CREATE TABLE IF NOT EXISTS night_availability_window(
            scope TEXT NOT NULL,
            scope_id TEXT NOT NULL,
            night_date TEXT NOT NULL,
            source TEXT NOT NULL,
            position INTEGER NOT NULL,
            start_ms INTEGER NOT NULL,
            end_ms INTEGER NOT NULL CHECK(end_ms>start_ms),
            PRIMARY KEY(scope,scope_id,night_date,source,position),
            FOREIGN KEY(scope,scope_id,night_date,source)
                REFERENCES night_availability(scope,scope_id,night_date,source) ON DELETE CASCADE);",
    )?;
    Ok(())
}

pub(crate) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT scope,scope_id,revision FROM night_availability_scope LIMIT 0",
        "SELECT scope,scope_id,night_date,source,state,note,revision,updated_at_ms FROM night_availability LIMIT 0",
        "SELECT scope,scope_id,night_date,source,position,start_ms,end_ms FROM night_availability_window LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    Ok(())
}

impl MetaStore {
    /// A scope's manual marks for the nights `from` to `to`, both included,
    /// with the revision a change must send back. A site or rig that does
    /// not exist is `NotFound`.
    pub fn night_marks(
        &self,
        scope: MarkScope,
        scope_id: Uuid,
        from: NightDate,
        to: NightDate,
    ) -> Result<NightMarks, Error> {
        // The scope revision and its marks are read by separate statements:
        // keep them on one snapshot, or a save that commits between them
        // shows marks newer than their scope, which reads as corruption.
        let snapshot = self.snapshot()?;
        let marks = read_marks(&snapshot, scope, scope_id, from, to)?;
        snapshot.end()?;
        Ok(marks)
    }

    /// Mark one or more nights in one transaction, replacing any manual mark
    /// they had. `expected_revision` is the scope revision the caller read;
    /// any other refuses the whole write with `Conflict`. Returns the marks
    /// written, at the new revision.
    pub fn save_night_marks(
        &mut self,
        scope: MarkScope,
        scope_id: Uuid,
        expected_revision: u64,
        edits: &[MarkEdit],
        now_ms: u64,
    ) -> Result<NightMarks, Error> {
        if edits.is_empty() || edits.len() > MAX_NIGHTS_PER_WRITE || now_ms > MAX_TIME_MS {
            return Err(Error::InvalidInput);
        }
        let edits = edits
            .iter()
            .map(MarkEdit::normalized)
            .collect::<Result<Vec<_>, _>>()?;
        let mut dates: Vec<NightDate> = edits.iter().map(|edit| edit.date).collect();
        dates.sort();
        dates.dedup();
        if dates.len() != edits.len() {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = bump(&tx, scope, scope_id, expected_revision)?;
        let mut marks = Vec::with_capacity(edits.len());
        for edit in edits {
            delete_mark(&tx, scope, scope_id, edit.date)?;
            tx.execute(
                "INSERT INTO night_availability(scope,scope_id,night_date,source,state,note,revision,updated_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    scope.as_column(),
                    scope_id.to_string(),
                    edit.date.to_string(),
                    MANUAL,
                    edit.state.as_column(),
                    edit.note,
                    to_sql(revision)?,
                    to_sql(now_ms)?
                ],
            )?;
            for (position, window) in edit.windows.iter().enumerate() {
                tx.execute(
                    "INSERT INTO night_availability_window(scope,scope_id,night_date,source,position,start_ms,end_ms)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        scope.as_column(),
                        scope_id.to_string(),
                        edit.date.to_string(),
                        MANUAL,
                        position as i64,
                        to_sql(window.start_ms)?,
                        to_sql(window.end_ms)?
                    ],
                )?;
            }
            marks.push(NightMark {
                date: edit.date,
                state: edit.state,
                windows: edit.windows,
                note: edit.note,
                revision,
                updated_at_ms: now_ms,
            });
        }
        tx.commit()?;
        marks.sort_by_key(|mark| mark.date);
        Ok(NightMarks {
            scope,
            scope_id,
            revision,
            marks,
        })
    }

    /// Return nights to the default, available unless a forecast says
    /// otherwise. Clearing a night with no mark is not an error. Returns the
    /// new scope revision.
    pub fn clear_night_marks(
        &mut self,
        scope: MarkScope,
        scope_id: Uuid,
        expected_revision: u64,
        dates: &[NightDate],
    ) -> Result<u64, Error> {
        if dates.is_empty() || dates.len() > MAX_NIGHTS_PER_WRITE {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = bump(&tx, scope, scope_id, expected_revision)?;
        for date in dates {
            delete_mark(&tx, scope, scope_id, *date)?;
        }
        tx.commit()?;
        Ok(revision)
    }
}

/// Check the scope exists and its revision is the expected one, and move it
/// on by one.
fn bump(
    tx: &Connection,
    scope: MarkScope,
    scope_id: Uuid,
    expected_revision: u64,
) -> Result<u64, Error> {
    if read_named(tx, scope.kind(), scope_id)?.is_none() {
        return Err(Error::NotFound);
    }
    let current = scope_revision(tx, scope, scope_id)?;
    if current != expected_revision {
        return Err(Error::Conflict);
    }
    let next = current.checked_add(1).ok_or(Error::Conflict)?;
    tx.execute(
        "INSERT INTO night_availability_scope(scope,scope_id,revision) VALUES(?1,?2,?3)
         ON CONFLICT(scope,scope_id) DO UPDATE SET revision=excluded.revision",
        params![scope.as_column(), scope_id.to_string(), to_sql(next)?],
    )?;
    Ok(next)
}

fn delete_mark(
    tx: &Connection,
    scope: MarkScope,
    scope_id: Uuid,
    date: NightDate,
) -> Result<(), Error> {
    let key = params![
        scope.as_column(),
        scope_id.to_string(),
        date.to_string(),
        MANUAL
    ];
    tx.execute(
        "DELETE FROM night_availability_window WHERE scope=?1 AND scope_id=?2 AND night_date=?3 AND source=?4",
        key,
    )?;
    tx.execute(
        "DELETE FROM night_availability WHERE scope=?1 AND scope_id=?2 AND night_date=?3 AND source=?4",
        key,
    )?;
    Ok(())
}

fn scope_revision(conn: &Connection, scope: MarkScope, scope_id: Uuid) -> Result<u64, Error> {
    let revision: Option<i64> = conn
        .query_row(
            "SELECT revision FROM night_availability_scope WHERE scope=?1 AND scope_id=?2",
            params![scope.as_column(), scope_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    match revision {
        None => Ok(0),
        Some(value) if value > 0 => Ok(value as u64),
        Some(_) => Err(Error::CorruptDatabase),
    }
}

fn read_marks(
    conn: &Connection,
    scope: MarkScope,
    scope_id: Uuid,
    from: NightDate,
    to: NightDate,
) -> Result<NightMarks, Error> {
    valid_id(scope_id)?;
    if from > to {
        return Err(Error::InvalidInput);
    }
    if read_named(conn, scope.kind(), scope_id)?.is_none() {
        return Err(Error::NotFound);
    }
    let revision = scope_revision(conn, scope, scope_id)?;
    let mut statement = conn.prepare(
        "SELECT night_date,state,note,revision,updated_at_ms FROM night_availability
         WHERE scope=?1 AND scope_id=?2 AND source=?3 AND night_date>=?4 AND night_date<=?5
         ORDER BY night_date LIMIT ?6",
    )?;
    let rows = statement
        .query_map(
            params![
                scope.as_column(),
                scope_id.to_string(),
                MANUAL,
                from.to_string(),
                to.to_string(),
                (MAX_NIGHTS_PER_READ + 1) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > MAX_NIGHTS_PER_READ {
        return Err(Error::InvalidInput);
    }
    let mut windows = conn.prepare(
        "SELECT start_ms,end_ms FROM night_availability_window
         WHERE scope=?1 AND scope_id=?2 AND night_date=?3 AND source=?4 ORDER BY position",
    )?;
    let mut marks = Vec::with_capacity(rows.len());
    for (date, state, note, mark_revision, updated_at_ms) in rows {
        let date: NightDate = date.parse().map_err(|_| Error::CorruptDatabase)?;
        let state: Availability = state.parse().map_err(|_| Error::CorruptDatabase)?;
        let found = windows
            .query_map(
                params![
                    scope.as_column(),
                    scope_id.to_string(),
                    date.to_string(),
                    MANUAL
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let found = found
            .into_iter()
            .map(|(start, end)| {
                Ok(Interval {
                    start_ms: u64::try_from(start).map_err(|_| Error::CorruptDatabase)?,
                    end_ms: u64::try_from(end).map_err(|_| Error::CorruptDatabase)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mark = NightMark {
            date,
            state,
            windows: found,
            note,
            revision: u64::try_from(mark_revision).map_err(|_| Error::CorruptDatabase)?,
            updated_at_ms: u64::try_from(updated_at_ms).map_err(|_| Error::CorruptDatabase)?,
        };
        let edit = MarkEdit {
            date: mark.date,
            state: mark.state,
            windows: mark.windows.clone(),
            note: mark.note.clone(),
        };
        if edit.normalized().map_err(|_| Error::CorruptDatabase)? != edit
            || mark.revision == 0
            || mark.revision > revision
        {
            return Err(Error::CorruptDatabase);
        }
        marks.push(mark);
    }
    Ok(NightMarks {
        scope,
        scope_id,
        revision,
        marks,
    })
}

fn to_sql(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, MetaStore, Uuid, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let rig = Uuid::new_v4();
        store.create_rig(rig, "North").unwrap();
        let site = Uuid::new_v4();
        store.create_site(site, "Backyard").unwrap();
        (dir, store, rig, site)
    }

    fn date(text: &str) -> NightDate {
        text.parse().unwrap()
    }

    fn edit(text: &str, state: Availability, windows: Vec<Interval>) -> MarkEdit {
        MarkEdit {
            date: date(text),
            state,
            windows,
            note: String::new(),
        }
    }

    #[test]
    fn marks_round_trip_and_a_stale_revision_is_refused() {
        let (_dir, mut store, rig, site) = store();
        let empty = store
            .night_marks(MarkScope::Rig, rig, date("2026-11-01"), date("2026-11-30"))
            .unwrap();
        assert_eq!((empty.revision, empty.marks.len()), (0, 0));
        // A range of nights the rig is down, written at once.
        let down: Vec<MarkEdit> = (3..=9)
            .map(|day| MarkEdit {
                note: "camera away ".into(),
                ..edit(
                    &format!("2026-11-0{day}"),
                    Availability::Unavailable,
                    vec![],
                )
            })
            .collect();
        let written = store
            .save_night_marks(MarkScope::Rig, rig, 0, &down, 1_000)
            .unwrap();
        assert_eq!(written.revision, 1);
        assert_eq!(written.marks.len(), 7);
        assert_eq!(written.marks[0].note, "camera away");
        // A limited night, its windows overlapping, stored merged.
        let limited = edit(
            "2026-11-12",
            Availability::Limited,
            vec![
                Interval {
                    start_ms: 5_000,
                    end_ms: 9_000,
                },
                Interval {
                    start_ms: 1_000,
                    end_ms: 6_000,
                },
            ],
        );
        assert_eq!(
            store
                .save_night_marks(
                    MarkScope::Rig,
                    rig,
                    0,
                    std::slice::from_ref(&limited),
                    2_000
                )
                .unwrap_err()
                .to_string(),
            Error::Conflict.to_string()
        );
        store
            .save_night_marks(MarkScope::Rig, rig, 1, &[limited], 2_000)
            .unwrap();
        let read = store
            .night_marks(MarkScope::Rig, rig, date("2026-11-01"), date("2026-11-30"))
            .unwrap();
        assert_eq!(read.revision, 2);
        assert_eq!(read.marks.len(), 8);
        let last = read.marks.last().unwrap();
        assert_eq!(last.state, Availability::Limited);
        assert_eq!(
            last.windows,
            vec![Interval {
                start_ms: 1_000,
                end_ms: 9_000
            }]
        );
        assert_eq!((last.revision, read.marks[0].revision), (2, 1));
        // Clearing returns nights to the default; a night with no mark is fine.
        let revision = store
            .clear_night_marks(
                MarkScope::Rig,
                rig,
                2,
                &[date("2026-11-03"), date("2026-11-20")],
            )
            .unwrap();
        assert_eq!(revision, 3);
        let read = store
            .night_marks(MarkScope::Rig, rig, date("2026-11-03"), date("2026-11-04"))
            .unwrap();
        assert_eq!(
            read.marks
                .iter()
                .map(|m| m.date.to_string())
                .collect::<Vec<_>>(),
            ["2026-11-04"]
        );
        // Each scope keeps its own marks and revision.
        let sky = store
            .night_marks(
                MarkScope::Site,
                site,
                date("2026-11-01"),
                date("2026-11-30"),
            )
            .unwrap();
        assert_eq!((sky.revision, sky.marks.len()), (0, 0));
        store
            .save_night_marks(
                MarkScope::Site,
                site,
                0,
                &[edit("2026-11-04", Availability::Available, vec![])],
                3_000,
            )
            .unwrap();
        assert!(matches!(
            store.night_marks(MarkScope::Site, rig, date("2026-11-01"), date("2026-11-30")),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn a_schema_32_store_gains_the_tables_and_keeps_its_rigs() {
        let (dir, store, rig, _site) = store();
        drop(store);
        let path = dir.path().join("meta.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE night_availability_window; DROP TABLE night_availability;
             DROP TABLE night_availability_scope; PRAGMA user_version=32;",
        )
        .unwrap();
        drop(conn);
        let mut store = MetaStore::open(&path).unwrap();
        assert!(store.rig(rig).unwrap().is_some());
        store
            .save_night_marks(
                MarkScope::Rig,
                rig,
                0,
                &[edit("2026-11-05", Availability::Unavailable, vec![])],
                1,
            )
            .unwrap();
    }

    #[test]
    fn malformed_marks_are_refused() {
        let (_dir, mut store, rig, _site) = store();
        let window = Interval {
            start_ms: 1_000,
            end_ms: 2_000,
        };
        for bad in [
            vec![edit("2026-11-01", Availability::Limited, vec![])],
            vec![edit("2026-11-01", Availability::Unavailable, vec![window])],
            vec![edit(
                "2026-11-01",
                Availability::Limited,
                vec![Interval {
                    start_ms: 2_000,
                    end_ms: 2_000,
                }],
            )],
            vec![MarkEdit {
                note: "a\nb".into(),
                ..edit("2026-11-01", Availability::Available, vec![])
            }],
            vec![MarkEdit {
                note: "x".repeat(MAX_NOTE_CHARS + 1),
                ..edit("2026-11-01", Availability::Available, vec![])
            }],
            vec![
                edit("2026-11-01", Availability::Available, vec![]),
                edit("2026-11-01", Availability::Unavailable, vec![]),
            ],
            vec![],
        ] {
            assert!(
                matches!(
                    store.save_night_marks(MarkScope::Rig, rig, 0, &bad, 1),
                    Err(Error::InvalidInput)
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            store.night_marks(MarkScope::Rig, rig, date("2026-11-02"), date("2026-11-01")),
            Err(Error::InvalidInput)
        ));
        assert_eq!("rig".parse::<MarkScope>().unwrap(), MarkScope::Rig);
        assert_eq!(MarkScope::Site.to_string(), "site");
        assert_eq!(
            "limited".parse::<Availability>().unwrap(),
            Availability::Limited
        );
        assert!("cloudy".parse::<Availability>().is_err());
    }
}
