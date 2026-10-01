//! Client evidence is staged separately from operator-approved planning setup.
use super::*;
use crate::profile::{Reported, RigProfile, Source};
use psf_guard_director_core::program::Configuration;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EquipmentReport {
    pub report_id: Uuid,
    pub client_id: Uuid,
    pub catalog_id: Uuid,
    pub rig_id: Uuid,
    pub profile_id: Uuid,
    pub observed_at_ms: u64,
    pub received_at_ms: u64,
    pub configuration: Configuration,
    pub filter_names: BTreeMap<String, String>,
    pub accepted_revision: Option<u64>,
    pub accepted_from_revision: Option<u64>,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch("CREATE TABLE equipment_report(client_id TEXT PRIMARY KEY NOT NULL REFERENCES director_client(id) ON DELETE CASCADE, report_id TEXT UNIQUE NOT NULL, payload TEXT NOT NULL);")?;
    Ok(())
}

fn validate(report: &EquipmentReport) -> Result<(), Error> {
    for id in [
        report.report_id,
        report.client_id,
        report.catalog_id,
        report.rig_id,
        report.profile_id,
    ] {
        valid_id(id)?;
    }
    if report.observed_at_ms == 0
        || report.observed_at_ms > 4_102_444_800_000
        || report.received_at_ms == 0
        || report.received_at_ms > 4_102_444_800_000
        || report.accepted_revision == Some(0)
        || report.accepted_revision.is_some() != report.accepted_from_revision.is_some()
        || report.observed_at_ms > report.received_at_ms.saturating_add(30_000)
        || report.received_at_ms.saturating_sub(report.observed_at_ms) > 900_000
    {
        return Err(Error::InvalidInput);
    }
    let mut profile = RigProfile::empty(report.rig_id, report.received_at_ms);
    profile.configuration = Some(Reported {
        value: report.configuration.clone(),
        source: Source::Plugin {},
        reported_at_ms: report.observed_at_ms,
    });
    profile.filter_names = report.filter_names.clone();
    if report.filter_names.is_empty() {
        return Err(Error::InvalidInput);
    }
    crate::profile::validate_profile(&profile)
}

fn live(conn: &Connection, report: &EquipmentReport) -> Result<(), Error> {
    let found: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM director_client c JOIN catalog_rig b ON b.catalog_id=c.catalog_id AND b.rig_id=c.rig_id WHERE c.id=?1 AND c.catalog_id=?2 AND c.rig_id=?3 AND c.profile_id=?4)",
        params![report.client_id.to_string(), report.catalog_id.to_string(), report.rig_id.to_string(), report.profile_id.to_string()], |r| r.get(0))?;
    if found {
        Ok(())
    } else {
        Err(Error::Conflict)
    }
}

fn read(conn: &Connection, client: Uuid) -> Result<Option<EquipmentReport>, Error> {
    let row: Option<(String, Vec<u8>)> = conn.query_row("SELECT report_id,substr(CAST(payload AS BLOB),1,?2) FROM equipment_report WHERE client_id=?1",
        params![client.to_string(), (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    row.map(|(id, bytes)| {
        let value: EquipmentReport = crate::configuration::decode(bytes)?;
        validate(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.client_id != client || value.report_id.to_string() != id {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}

impl MetaStore {
    pub fn report_equipment(&mut self, report: &EquipmentReport) -> Result<EquipmentReport, Error> {
        validate(report)?;
        if report.accepted_revision.is_some() || report.accepted_from_revision.is_some() {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        live(&tx, report)?;
        if let Some(mut old) = read(&tx, report.client_id)? {
            if old.report_id == report.report_id {
                let saved = old.clone();
                old.received_at_ms = report.received_at_ms;
                old.accepted_revision = None;
                old.accepted_from_revision = None;
                if old != *report {
                    return Err(Error::Conflict);
                }
                tx.commit()?;
                return Ok(saved);
            }
            if old.observed_at_ms >= report.observed_at_ms {
                return Err(Error::Conflict);
            }
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM equipment_report WHERE report_id=?1)",
            [report.report_id.to_string()],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Conflict);
        }
        tx.execute("INSERT INTO equipment_report(client_id,report_id,payload) VALUES(?1,?2,?3) ON CONFLICT(client_id) DO UPDATE SET report_id=excluded.report_id,payload=excluded.payload",
            params![report.client_id.to_string(), report.report_id.to_string(), crate::configuration::encode(report)?])?;
        tx.commit()?;
        Ok(report.clone())
    }

    pub fn equipment_reports(&self, rig: Uuid) -> Result<Vec<EquipmentReport>, Error> {
        valid_id(rig)?;
        let mut stmt = self.connection.prepare("SELECT r.client_id FROM equipment_report r JOIN director_client c ON c.id=r.client_id WHERE c.rig_id=?1 ORDER BY r.client_id LIMIT 256")?;
        let ids: Vec<String> = stmt
            .query_map([rig.to_string()], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        ids.into_iter()
            .map(|id| {
                let report =
                    read(&self.connection, parse_id(&id)?)?.ok_or(Error::CorruptDatabase)?;
                live(&self.connection, &report)?;
                Ok(report)
            })
            .collect()
    }

    /// Review and profile CAS share a transaction. A pending report cannot
    /// replace an outstanding allocation's equipment assumptions.
    pub fn accept_equipment_report(
        &mut self,
        rig: Uuid,
        client: Uuid,
        report_id: Uuid,
        expected_revision: u64,
        now: u64,
    ) -> Result<RigProfile, Error> {
        for id in [rig, client, report_id] {
            valid_id(id)?;
        }
        if now == 0 || now > 4_102_444_800_000 {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = read(&tx, client)?.ok_or(Error::NotFound)?;
        live(&tx, &report)?;
        if report.rig_id != rig || report.report_id != report_id {
            return Err(Error::Conflict);
        }
        let mut profile =
            crate::profile::read_profile(&tx, rig)?.unwrap_or_else(|| RigProfile::empty(rig, now));
        if let Some(revision) = report.accepted_revision {
            if report.accepted_from_revision == Some(expected_revision)
                && profile.revision == revision
            {
                tx.commit()?;
                return Ok(profile);
            }
            return Err(Error::Conflict);
        }
        if now < report.received_at_ms
            || now.saturating_sub(report.observed_at_ms.min(report.received_at_ms)) > 900_000
            || tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM execution_allocation WHERE rig_id=?1)",
                [rig.to_string()],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(Error::Conflict);
        }
        profile.configuration = Some(Reported {
            value: report.configuration.clone(),
            source: Source::Plugin {},
            reported_at_ms: report.observed_at_ms,
        });
        profile.filter_names = report.filter_names.clone();
        profile.updated_at_ms = now;
        let saved = crate::profile::save_profile(&tx, &profile, expected_revision)?;
        report.accepted_revision = Some(saved.revision);
        report.accepted_from_revision = Some(expected_revision);
        tx.execute(
            "UPDATE equipment_report SET payload=?2 WHERE client_id=?1",
            params![client.to_string(), crate::configuration::encode(&report)?],
        )?;
        tx.commit()?;
        Ok(saved)
    }
}
