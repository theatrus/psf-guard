use super::*;
use psf_guard_director_core::priority::ActiveGoal;
use sha2::{Digest, Sha256};

pub(super) fn create(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch("CREATE TABLE observing_selection(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32));")?;
    connection.execute(
        "INSERT INTO observing_selection VALUES(1,'null',?1)",
        params![Sha256::digest(b"null").as_slice()],
    )?;
    Ok(())
}

pub(super) fn read(connection: &Connection) -> Result<Option<ActiveGoal>, Error> {
    let row: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT substr(payload,1,4097),digest FROM observing_selection WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (payload, digest) = row.ok_or(Error::CorruptLedger)?;
    if payload.len() > 4096 || Sha256::digest(payload.as_bytes()).as_slice() != digest {
        return Err(Error::CorruptLedger);
    }
    serde_json::from_str(&payload).map_err(|_| Error::CorruptLedger)
}

pub(super) fn selected(connection: &Connection, goal_id: &str, now_ms: u64) -> Result<(), Error> {
    if let Some(previous) = read(connection)? {
        if now_ms < previous.selected_at_ms {
            return Err(Error::InvalidInput);
        }
        if previous.goal_id == goal_id {
            return Ok(());
        }
    }
    let payload = serde_json::to_string(&ActiveGoal {
        goal_id: goal_id.into(),
        selected_at_ms: now_ms,
    })?;
    connection.execute("INSERT INTO observing_selection VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload,digest=excluded.digest",
        params![payload, Sha256::digest(payload.as_bytes()).as_slice()])?;
    Ok(())
}
