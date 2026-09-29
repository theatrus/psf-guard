//! The exposure template library: Director's own templates, kept in the
//! meta store so any rig can shoot with them. A library template names a
//! filter and camera settings, not a database; activation writes one into
//! a rig database that has nothing like it. Nothing here touches a rig.

use super::*;
use psf_guard_director_core::MAX_REQUEST_BYTES;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposureTemplate {
    pub id: Uuid,
    /// 0 for a template not saved yet; the store sets 1 on first save.
    pub revision: u64,
    pub name: String,
    /// The filter as the rig names it; the bandpass is read from it.
    pub filter_name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub gain: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub offset: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub bin: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub readout_mode: Option<i32>,
    /// The exposure a plan starts from when this template is chosen.
    pub default_exposure_seconds: f64,
    pub updated_at_ms: u64,
}

const MAX_TEMPLATES: i64 = 512;
const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

fn text_ok(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(crate) fn validate_template(template: &ExposureTemplate) -> Result<(), Error> {
    valid_id(template.id)?;
    if !text_ok(&template.name, 256)
        || !text_ok(&template.filter_name, 128)
        || template.gain.is_some_and(|value| !(0..=100_000).contains(&value))
        || template.offset.is_some_and(|value| !(0..=100_000).contains(&value))
        || template.bin.is_some_and(|value| !(1..=8).contains(&value))
        || template
            .readout_mode
            .is_some_and(|value| !(0..=64).contains(&value))
        || !template.default_exposure_seconds.is_finite()
        || template.default_exposure_seconds <= 0.0
        || template.default_exposure_seconds > 86_400.0
        || template.updated_at_ms > MAX_TIME_MS
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

impl MetaStore {
    /// Every library template, by filter and then name.
    pub fn templates(&self) -> Result<Vec<ExposureTemplate>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT substr(CAST(payload AS BLOB),1,?1) FROM exposure_template
             ORDER BY lower(filter_name), lower(name), id",
        )?;
        let rows = statement.query_map([(MAX_REQUEST_BYTES + 1) as i64], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        let mut templates = Vec::new();
        for payload in rows {
            let template: ExposureTemplate = super::configuration::decode(payload?)?;
            validate_template(&template).map_err(|_| Error::CorruptDatabase)?;
            templates.push(template);
        }
        Ok(templates)
    }

    pub fn template(&self, id: Uuid) -> Result<Option<ExposureTemplate>, Error> {
        read_template(&self.connection, id)
    }

    /// Save with compare-and-set: `expected_revision` is the revision the
    /// caller read, 0 for a new template. Saving what is already there is
    /// not a new revision.
    pub fn save_template(
        &mut self,
        template: &ExposureTemplate,
        expected_revision: u64,
    ) -> Result<ExposureTemplate, Error> {
        validate_template(template)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = read_template(&tx, template.id)?;
        let current = stored.as_ref().map_or(0, |value| value.revision);
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        if stored.is_none() {
            let count: i64 =
                tx.query_row("SELECT count(*) FROM exposure_template", [], |row| row.get(0))?;
            if count >= MAX_TEMPLATES {
                return Err(Error::InvalidInput);
            }
        }
        let mut next = template.clone();
        next.revision = current;
        if let Some(stored) = stored {
            let mut same = stored.clone();
            same.updated_at_ms = next.updated_at_ms;
            if same == next {
                tx.commit()?;
                return Ok(stored);
            }
        }
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
        let payload = super::configuration::encode(&next)?;
        tx.execute(
            "INSERT INTO exposure_template(id,revision,filter_name,name,payload) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET revision=excluded.revision, filter_name=excluded.filter_name,
             name=excluded.name, payload=excluded.payload",
            params![
                next.id.to_string(),
                i64::try_from(next.revision).map_err(|_| Error::Conflict)?,
                next.filter_name,
                next.name,
                payload
            ],
        )?;
        tx.commit()?;
        Ok(next)
    }

    /// Remove a template the caller has read at `expected_revision`. Plans
    /// that chose it keep their copy of its settings.
    pub fn delete_template(&mut self, id: Uuid, expected_revision: u64) -> Result<(), Error> {
        valid_id(id)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = read_template(&tx, id)?.ok_or(Error::NotFound)?;
        if stored.revision != expected_revision {
            return Err(Error::Conflict);
        }
        tx.execute(
            "DELETE FROM exposure_template WHERE id=?1",
            [id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn read_template(conn: &Connection, id: Uuid) -> Result<Option<ExposureTemplate>, Error> {
    valid_id(id)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM exposure_template WHERE id=?1",
            params![id.to_string(), (MAX_REQUEST_BYTES + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let value: ExposureTemplate = super::configuration::decode(payload)?;
        validate_template(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.id != id || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS exposure_template(
            id TEXT PRIMARY KEY NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            filter_name TEXT NOT NULL,
            name TEXT NOT NULL,
            payload TEXT NOT NULL);",
    )?;
    Ok(())
}
