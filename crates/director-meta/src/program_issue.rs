//! Stable preview issuance, not an acquisition lease or an execution budget.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramIssue {
    pub id: Uuid,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE program_issue(
            rig_id TEXT PRIMARY KEY NOT NULL REFERENCES rig(id),
            catalog_id TEXT NOT NULL REFERENCES catalog(id),
            fingerprint TEXT NOT NULL, id TEXT NOT NULL,
            issued_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL);",
    )?;
    Ok(())
}

impl MetaStore {
    /// Persist one current preview per rig. Reverting inputs still creates a new
    /// identity. An expired preview is never extended under its old identity.
    pub fn issue_program_preview(
        &mut self,
        rig: Uuid,
        catalog: Uuid,
        fingerprint: &str,
        now_ms: u64,
        lifetime_ms: u64,
    ) -> Result<ProgramIssue, Error> {
        valid_id(rig)?;
        valid_id(catalog)?;
        if fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || lifetime_ms == 0
        {
            return Err(Error::InvalidInput);
        }
        let expires = now_ms.checked_add(lifetime_ms).ok_or(Error::InvalidInput)?;
        let now = i64::try_from(now_ms).map_err(|_| Error::InvalidInput)?;
        let expires = i64::try_from(expires).map_err(|_| Error::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_rig WHERE rig_id=?1 AND catalog_id=?2)",
            params![rig.to_string(), catalog.to_string()],
            |r| r.get(0),
        )?;
        if !bound {
            return Err(Error::NotFound);
        }
        let old = tx
            .query_row(
                "SELECT id,issued_at_ms,expires_at_ms FROM program_issue
             WHERE rig_id=?1 AND catalog_id=?2 AND fingerprint=?3",
                params![rig.to_string(), catalog.to_string(), fingerprint],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((id, issued, expires)) = old
            && issued >= 0
            && issued <= now
            && now < expires
        {
            let result = ProgramIssue {
                id: parse_id(&id)?,
                issued_at_ms: issued as u64,
                expires_at_ms: expires as u64,
            };
            tx.commit()?;
            return Ok(result);
        }
        let id = Uuid::new_v4();
        tx.execute(
            "INSERT INTO program_issue VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(rig_id) DO UPDATE SET catalog_id=excluded.catalog_id,
             fingerprint=excluded.fingerprint,id=excluded.id,
             issued_at_ms=excluded.issued_at_ms,expires_at_ms=excluded.expires_at_ms",
            params![
                rig.to_string(),
                catalog.to_string(),
                fingerprint,
                id.to_string(),
                now,
                expires
            ],
        )?;
        tx.commit()?;
        Ok(ProgramIssue {
            id,
            issued_at_ms: now_ms,
            expires_at_ms: expires as u64,
        })
    }
}
