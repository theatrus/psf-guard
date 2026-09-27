//! Director-only credentials. Callers generate random secrets and pass hashes;
//! neither pairing codes nor bearer secrets are stored here.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Client {
    pub client_id: Uuid,
    pub catalog_id: Uuid,
    pub rig_id: Uuid,
    pub profile_id: Uuid,
    pub client_name: String,
    pub created_at_ms: i64,
}

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE director_pairing(
            token_hash TEXT PRIMARY KEY NOT NULL,
            catalog_id TEXT NOT NULL REFERENCES catalog(id),
            rig_id TEXT UNIQUE NOT NULL REFERENCES rig(id),
            expires_at_ms INTEGER NOT NULL);
         CREATE TABLE director_client(
            id TEXT PRIMARY KEY NOT NULL,
            catalog_id TEXT NOT NULL REFERENCES catalog(id),
            rig_id TEXT NOT NULL REFERENCES rig(id),
            profile_id TEXT NOT NULL,
            name TEXT NOT NULL,
            token_hash TEXT UNIQUE NOT NULL,
            created_at_ms INTEGER NOT NULL);
         CREATE INDEX director_client_rig ON director_client(rig_id,id);",
    )?;
    Ok(())
}

fn valid_hash(hash: &str) -> Result<(), Error> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

fn bound(conn: &Connection, catalog: Uuid, rig: Uuid) -> Result<(), Error> {
    valid_id(catalog)?;
    valid_id(rig)?;
    if super::catalog_rig::read_binding(conn, catalog)? != Some(rig) {
        return Err(Error::Conflict);
    }
    Ok(())
}

impl MetaStore {
    /// Replace the outstanding code for this rig. Issuance does not revoke
    /// existing clients; operators revoke those explicitly by client ID.
    pub fn issue_client_pairing(
        &mut self,
        catalog: Uuid,
        rig: Uuid,
        hash: &str,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<(), Error> {
        valid_hash(hash)?;
        if now_ms < 0 || expires_at_ms <= now_ms || expires_at_ms - now_ms > 3_600_000 {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        bound(&tx, catalog, rig)?;
        tx.execute(
            "DELETE FROM director_pairing WHERE rig_id=?1 OR expires_at_ms<=?2",
            params![rig.to_string(), now_ms],
        )?;
        tx.execute(
            "INSERT INTO director_pairing VALUES(?1,?2,?3,?4)",
            params![hash, catalog.to_string(), rig.to_string(), expires_at_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Consuming the code and storing the client are one durable transaction.
    /// A failed write leaves the code usable; a lost successful response needs
    /// a new operator-issued code, never a replay of the secret response.
    pub fn redeem_client_pairing(
        &mut self,
        pairing_hash: &str,
        token_hash: &str,
        profile: Uuid,
        name: &str,
        now_ms: i64,
    ) -> Result<Client, Error> {
        valid_hash(pairing_hash)?;
        valid_hash(token_hash)?;
        valid_id(profile)?;
        valid_name(name)?;
        if name.len() > 80 || now_ms < 0 {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String,String)> = tx.query_row(
            "SELECT catalog_id,rig_id FROM director_pairing WHERE token_hash=?1 AND expires_at_ms>?2",
            params![pairing_hash,now_ms], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (catalog, rig) = row.ok_or(Error::NotFound)?;
        let catalog = parse_id(&catalog)?;
        let rig = parse_id(&rig)?;
        bound(&tx, catalog, rig)?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM director_client WHERE rig_id=?1",
            [rig.to_string()],
            |r| r.get(0),
        )?;
        if count >= 256 {
            return Err(Error::Conflict);
        }
        let result = Client {
            client_id: Uuid::new_v4(),
            catalog_id: catalog,
            rig_id: rig,
            profile_id: profile,
            client_name: name.into(),
            created_at_ms: now_ms,
        };
        tx.execute(
            "INSERT INTO director_client VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                result.client_id.to_string(),
                catalog.to_string(),
                rig.to_string(),
                profile.to_string(),
                name,
                token_hash,
                now_ms
            ],
        )?;
        tx.execute(
            "DELETE FROM director_pairing WHERE token_hash=?1",
            [pairing_hash],
        )?;
        tx.commit()?;
        Ok(result)
    }

    /// Every authentication rechecks the live rig/catalog binding and revocation.
    pub fn client_for_token(&self, hash: &str) -> Result<Option<Client>, Error> {
        valid_hash(hash)?;
        self.connection.query_row(
            "SELECT c.id,c.catalog_id,c.rig_id,c.profile_id,c.name,c.created_at_ms FROM director_client c
             JOIN catalog_rig b ON b.catalog_id=c.catalog_id AND b.rig_id=c.rig_id WHERE c.token_hash=?1",
            [hash], read_client).optional().map_err(Error::from)
    }

    pub fn clients(&self, rig: Uuid) -> Result<Vec<Client>, Error> {
        valid_id(rig)?;
        let mut stmt = self.connection.prepare("SELECT id,catalog_id,rig_id,profile_id,name,created_at_ms FROM director_client WHERE rig_id=?1 ORDER BY id LIMIT 256")?;
        let rows = stmt.query_map([rig.to_string()], read_client)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
    }

    pub fn revoke_client(&mut self, rig: Uuid, client: Uuid) -> Result<bool, Error> {
        valid_id(rig)?;
        valid_id(client)?;
        Ok(self.connection.execute(
            "DELETE FROM director_client WHERE rig_id=?1 AND id=?2",
            params![rig.to_string(), client.to_string()],
        )? == 1)
    }
}

fn read_client(row: &rusqlite::Row<'_>) -> rusqlite::Result<Client> {
    fn id(row: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Uuid> {
        let value: String = row.get(i)?;
        parse_id(&value).map_err(|_| rusqlite::Error::InvalidQuery)
    }
    Ok(Client {
        client_id: id(row, 0)?,
        catalog_id: id(row, 1)?,
        rig_id: id(row, 2)?,
        profile_id: id(row, 3)?,
        client_name: row.get(4)?,
        created_at_ms: row.get(5)?,
    })
}
