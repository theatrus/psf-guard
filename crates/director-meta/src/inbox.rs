//! The coordinator's inbox for a rig: durable execution receipts by ledger
//! and sequence, the contiguous cursor it can acknowledge, and the rig's last
//! coalesced live status. Receipts are evidence of what a rig did; they grant
//! nothing and are never rewritten once stored.

use super::*;
use psf_guard_director_core::MAX_REQUEST_BYTES;

pub const MAX_PAGE: usize = 256;
const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

/// One stored receipt. The payload is the executor's event as sent, kept
/// verbatim so a later reader can apply rules this version does not know.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub rig_id: Uuid,
    pub ledger_id: String,
    pub sequence: u64,
    /// The goal the attempt served; the program's exposure plan GUID.
    pub goal_id: String,
    pub capture_id: String,
    /// `reserved`, `saved`, `failed` or `uncertain`.
    pub state: String,
    pub payload: serde_json::Value,
    pub received_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FeedCursor {
    pub rig_id: Uuid,
    pub ledger_id: String,
    /// Every sequence up to and including this one is stored.
    pub highest_contiguous: u64,
    /// The largest sequence stored, which may sit past a gap.
    pub highest_seen: u64,
    pub last_checkin_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stored {
    Applied,
    Duplicate,
    /// Same ledger and sequence, different content. Kept out of the store.
    Conflict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RigStatus {
    pub rig_id: Uuid,
    pub session_id: String,
    pub reported_at_ms: u64,
    pub payload: serde_json::Value,
    pub received_at_ms: u64,
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|c| c.is_ascii_graphic())
}

fn payload_ok(value: &serde_json::Value) -> Result<String, Error> {
    let json = serde_json::to_string(value).map_err(|_| Error::InvalidInput)?;
    if json.len() > MAX_REQUEST_BYTES {
        return Err(Error::InvalidInput);
    }
    Ok(json)
}

impl MetaStore {
    /// Store one page of receipts from one ledger in one transaction. The
    /// answer says what happened to each and the cursor the caller may now
    /// acknowledge. A content conflict never blocks the receipts around it;
    /// a ledger owned by another rig rejects the entire page.
    pub fn store_receipts(
        &mut self,
        receipts: &[Receipt],
        now_ms: u64,
    ) -> Result<(Vec<Stored>, FeedCursor), Error> {
        if receipts.is_empty() || receipts.len() > MAX_PAGE || now_ms > MAX_TIME_MS {
            return Err(Error::InvalidInput);
        }
        let rig = receipts[0].rig_id;
        let ledger = receipts[0].ledger_id.clone();
        valid_id(rig)?;
        if !valid_text(&ledger, 128) {
            return Err(Error::InvalidInput);
        }
        let mut last = 0;
        for receipt in receipts {
            if receipt.rig_id != rig
                || receipt.ledger_id != ledger
                || receipt.sequence == 0
                || receipt.sequence <= last
                || receipt.sequence > i64::MAX as u64
                || !valid_text(&receipt.goal_id, 128)
                || !valid_text(&receipt.capture_id, 128)
                || !matches!(
                    receipt.state.as_str(),
                    "reserved" | "saved" | "failed" | "uncertain"
                )
            {
                return Err(Error::InvalidInput);
            }
            last = receipt.sequence;
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Rig, rig)?.is_none() {
            return Err(Error::NotFound);
        }
        // Ledger IDs are globally keyed. Check both projections before any
        // insert, including legacy feeds whose events/cursor disagree.
        let wrong_owner: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rig_feed WHERE ledger_id=?1 AND rig_id!=?2)
                 OR EXISTS(SELECT 1 FROM rig_event WHERE ledger_id=?1 AND rig_id!=?2)",
            params![ledger, rig.to_string()],
            |row| row.get(0),
        )?;
        if wrong_owner {
            return Err(Error::Conflict);
        }
        let mut outcomes = Vec::with_capacity(receipts.len());
        for receipt in receipts {
            let payload = payload_ok(&receipt.payload)?;
            let existing: Option<String> = tx
                .query_row(
                    "SELECT payload FROM rig_event WHERE ledger_id=?1 AND sequence=?2",
                    params![ledger, receipt.sequence as i64],
                    |row| row.get(0),
                )
                .optional()?;
            match existing {
                Some(stored) if stored == payload => outcomes.push(Stored::Duplicate),
                Some(_) => outcomes.push(Stored::Conflict),
                None => {
                    tx.execute(
                        "INSERT INTO rig_event(ledger_id,sequence,rig_id,goal_id,capture_id,state,payload,received_at_ms)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![
                            ledger,
                            receipt.sequence as i64,
                            rig.to_string(),
                            receipt.goal_id,
                            receipt.capture_id,
                            receipt.state,
                            payload,
                            now_ms as i64
                        ],
                    )?;
                    outcomes.push(Stored::Applied);
                }
            }
        }
        let cursor = recompute_cursor(&tx, rig, &ledger, now_ms)?;
        tx.commit()?;
        Ok((outcomes, cursor))
    }

    pub fn feed_cursors_for_rig(&self, rig: Uuid) -> Result<Vec<FeedCursor>, Error> {
        valid_id(rig)?;
        let mut statement = self.connection.prepare(
            "SELECT ledger_id, highest_contiguous, highest_seen, last_checkin_ms FROM rig_feed WHERE rig_id=?1 ORDER BY ledger_id",
        )?;
        let rows = statement
            .query_map([rig.to_string()], |row| {
                Ok(FeedCursor {
                    rig_id: rig,
                    ledger_id: row.get(0)?,
                    highest_contiguous: row.get::<_, i64>(1)? as u64,
                    highest_seen: row.get::<_, i64>(2)? as u64,
                    last_checkin_ms: row.get::<_, i64>(3)? as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn feed_cursor(&self, rig: Uuid, ledger: &str) -> Result<Option<FeedCursor>, Error> {
        valid_id(rig)?;
        read_cursor(&self.connection, rig, ledger)
    }

    /// Saved captures per goal that this coordinator has heard of, across
    /// every ledger the rig used. A projection input, not accepted credit.
    pub fn saved_captures_by_goal(&self, rig: Uuid) -> Result<Vec<(String, u32)>, Error> {
        valid_id(rig)?;
        let mut statement = self.connection.prepare(
            "SELECT goal_id, COUNT(DISTINCT capture_id) FROM rig_event WHERE rig_id=?1 AND state='saved' GROUP BY goal_id",
        )?;
        let rows = statement
            .query_map([rig.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(goal, count)| (goal, u32::try_from(count).unwrap_or(u32::MAX)))
            .collect())
    }

    /// Keep the newest status per rig. An older report for the same session,
    /// or any report from an older session, is refused rather than applied.
    pub fn record_status(&mut self, status: &RigStatus) -> Result<bool, Error> {
        valid_id(status.rig_id)?;
        if !valid_text(&status.session_id, 128)
            || status.reported_at_ms > MAX_TIME_MS
            || status.received_at_ms > MAX_TIME_MS
        {
            return Err(Error::InvalidInput);
        }
        let payload = payload_ok(&status.payload)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Rig, status.rig_id)?.is_none() {
            return Err(Error::NotFound);
        }
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT session_id, reported_at_ms FROM rig_status WHERE rig_id=?1",
                [status.rig_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((session, reported)) = current {
            let same_session = session == status.session_id;
            if (same_session && reported as u64 >= status.reported_at_ms)
                || (!same_session && reported as u64 > status.reported_at_ms)
            {
                tx.commit()?;
                return Ok(false);
            }
        }
        tx.execute(
            "INSERT INTO rig_status(rig_id,session_id,reported_at_ms,payload,received_at_ms) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(rig_id) DO UPDATE SET session_id=excluded.session_id, reported_at_ms=excluded.reported_at_ms,
             payload=excluded.payload, received_at_ms=excluded.received_at_ms",
            params![
                status.rig_id.to_string(),
                status.session_id,
                status.reported_at_ms as i64,
                payload,
                status.received_at_ms as i64
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn rig_status(&self, rig: Uuid) -> Result<Option<RigStatus>, Error> {
        valid_id(rig)?;
        read_status(&self.connection, rig)
    }

    pub fn rig_statuses(&self) -> Result<Vec<RigStatus>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT rig_id FROM rig_status ORDER BY rig_id")?;
        let rigs = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rigs.into_iter()
            .filter_map(|rig| {
                parse_id(&rig)
                    .and_then(|rig| read_status(&self.connection, rig))
                    .transpose()
            })
            .collect()
    }
}

fn recompute_cursor(
    conn: &Connection,
    rig: Uuid,
    ledger: &str,
    now_ms: u64,
) -> Result<FeedCursor, Error> {
    let mut statement = conn.prepare(
        "SELECT sequence FROM rig_event WHERE ledger_id=?1 AND rig_id=?2 ORDER BY sequence",
    )?;
    let sequences = statement
        .query_map(params![ledger, rig.to_string()], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut contiguous = 0u64;
    for sequence in &sequences {
        if *sequence as u64 == contiguous + 1 {
            contiguous += 1;
        } else {
            break;
        }
    }
    let highest_seen = sequences.last().copied().unwrap_or(0) as u64;
    conn.execute(
        "INSERT INTO rig_feed(ledger_id,rig_id,highest_contiguous,highest_seen,last_checkin_ms) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(ledger_id) DO UPDATE SET highest_contiguous=excluded.highest_contiguous, highest_seen=excluded.highest_seen, last_checkin_ms=excluded.last_checkin_ms",
        params![
            ledger,
            rig.to_string(),
            contiguous as i64,
            highest_seen as i64,
            now_ms as i64
        ],
    )?;
    Ok(FeedCursor {
        rig_id: rig,
        ledger_id: ledger.to_owned(),
        highest_contiguous: contiguous,
        highest_seen,
        last_checkin_ms: now_ms,
    })
}

fn read_cursor(conn: &Connection, rig: Uuid, ledger: &str) -> Result<Option<FeedCursor>, Error> {
    conn.query_row(
        "SELECT highest_contiguous, highest_seen, last_checkin_ms FROM rig_feed WHERE ledger_id=?1 AND rig_id=?2",
        params![ledger, rig.to_string()],
        |row| {
            Ok(FeedCursor {
                rig_id: rig,
                ledger_id: ledger.to_owned(),
                highest_contiguous: row.get::<_, i64>(0)? as u64,
                highest_seen: row.get::<_, i64>(1)? as u64,
                last_checkin_ms: row.get::<_, i64>(2)? as u64,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}

fn read_status(conn: &Connection, rig: Uuid) -> Result<Option<RigStatus>, Error> {
    let row: Option<(String, i64, String, i64)> = conn
        .query_row(
            "SELECT session_id, reported_at_ms, payload, received_at_ms FROM rig_status WHERE rig_id=?1",
            [rig.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(session_id, reported_at_ms, payload, received_at_ms)| {
        Ok(RigStatus {
            rig_id: rig,
            session_id,
            reported_at_ms: reported_at_ms as u64,
            payload: serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?,
            received_at_ms: received_at_ms as u64,
        })
    })
    .transpose()
}
