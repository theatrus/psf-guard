//! Reviewed imports and immutable contribution outbox, not local authority.
//! Hosts own authentication, source freshness, catalog evidence and transport.

use super::*;
use psf_guard_director_interop::{
    astrocollab::Source,
    collaboration::{
        decode_recorded, digest, exposure_matches, prepare_import, FinalizedContribution,
        PreparedImport, Recorded, MAX_REPORTS, MAX_REPORT_FRAMES,
    },
};
use std::collections::BTreeSet;

const MAX_TIME_MS: u64 = 4_102_444_800_000;

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS collaboration_project(
            base_url TEXT NOT NULL, remote_project_id TEXT NOT NULL,
            project_id TEXT UNIQUE NOT NULL REFERENCES global_project(id),
            PRIMARY KEY(base_url,remote_project_id));
         CREATE TABLE IF NOT EXISTS collaboration_import(
            id TEXT PRIMARY KEY NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id),
            project_id TEXT NOT NULL REFERENCES global_project(id),
            base_url TEXT NOT NULL, agent_id TEXT NOT NULL, night TEXT NOT NULL,
            task_id TEXT NOT NULL, digest TEXT NOT NULL, payload BLOB NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0), imported_at_ms INTEGER NOT NULL,
            UNIQUE(base_url,agent_id,night,task_id));
         CREATE INDEX IF NOT EXISTS collaboration_import_project ON collaboration_import(project_id,id);
         CREATE TABLE IF NOT EXISTS collaboration_revision(
            import_id TEXT NOT NULL REFERENCES collaboration_import(id), digest TEXT NOT NULL,
            payload BLOB NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
            imported_at_ms INTEGER NOT NULL, PRIMARY KEY(import_id,digest));
         CREATE TABLE IF NOT EXISTS collaboration_outbox(
            id TEXT PRIMARY KEY NOT NULL, import_id TEXT NOT NULL REFERENCES collaboration_import(id),
            panel INTEGER NOT NULL, filter TEXT NOT NULL, integration_ms INTEGER NOT NULL,
            source_digest TEXT NOT NULL, geometry_digest TEXT NOT NULL, payload TEXT NOT NULL, captures TEXT NOT NULL,
            images TEXT NOT NULL, digest TEXT NOT NULL, created_at_ms INTEGER NOT NULL,
            recorded TEXT, acknowledged_at_ms INTEGER,
            FOREIGN KEY(import_id,source_digest) REFERENCES collaboration_revision(import_id,digest));
         CREATE INDEX IF NOT EXISTS collaboration_outbox_pending ON collaboration_outbox(acknowledged_at_ms,created_at_ms,id);
         CREATE TABLE IF NOT EXISTS collaboration_capture(
            capture_id TEXT PRIMARY KEY NOT NULL, image_guid TEXT UNIQUE NOT NULL,
            import_id TEXT NOT NULL REFERENCES collaboration_import(id), panel INTEGER NOT NULL,
            filter TEXT NOT NULL);"
    )?;
    validate_tables(conn)
}

pub(crate) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT base_url,remote_project_id,project_id FROM collaboration_project LIMIT 0",
        "SELECT id,rig_id,project_id,base_url,agent_id,night,task_id,digest,payload,revision,imported_at_ms FROM collaboration_import LIMIT 0",
        "SELECT import_id,digest,payload,revision,imported_at_ms FROM collaboration_revision LIMIT 0",
        "SELECT id,import_id,panel,filter,integration_ms,source_digest,geometry_digest,payload,captures,images,digest,created_at_ms,recorded,acknowledged_at_ms FROM collaboration_outbox LIMIT 0",
        "SELECT capture_id,image_guid,import_id,panel,filter FROM collaboration_capture LIMIT 0",
    ] { conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?; }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct Imported {
    pub rig_id: Uuid,
    pub revision: u64,
    pub imported_at_ms: u64,
    pub plan: PreparedImport,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportAction {
    Create,
    Update,
    Unchanged,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportPreview {
    pub import_id: Uuid,
    pub project_id: Uuid,
    pub rig_id: Uuid,
    pub action: ImportAction,
    pub expected_revision: u64,
    pub review_digest: String,
    /// Imported snapshots and draft intent never activate a rig.
    pub acquisition_enabled: bool,
}

fn valid_time(now: u64) -> Result<(), Error> {
    if now > MAX_TIME_MS {
        Err(Error::InvalidInput)
    } else {
        Ok(())
    }
}

impl MetaStore {
    pub fn collaboration_import(&self, id: Uuid) -> Result<Option<Imported>, Error> {
        read_import(&self.connection, id)
    }

    pub fn collaboration_import_revision(
        &self,
        id: Uuid,
        digest: &str,
    ) -> Result<Option<Imported>, Error> {
        read_import_revision(&self.connection, id, digest)
    }

    pub fn collaboration_import_ids(
        &self,
        project: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<super::configuration::SnapshotIds, Error> {
        super::configuration::snapshot_ids(&self.connection,
            "SELECT id FROM collaboration_import WHERE project_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            project, after, limit)
    }

    pub fn preview_collaboration_import(
        &self,
        plan: &PreparedImport,
        rig: Uuid,
    ) -> Result<ImportPreview, Error> {
        preview(&self.connection, plan, rig)
    }

    /// Apply exactly the reviewed snapshot. The snapshot is safe to re-read
    /// offline, but supplies no recipe, budget or hardware authorization.
    pub fn apply_collaboration_import(
        &mut self,
        plan: &PreparedImport,
        rig: Uuid,
        review_digest: &str,
        now_ms: u64,
    ) -> Result<Imported, Error> {
        valid_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reviewed = preview(&tx, plan, rig)?;
        if reviewed.review_digest != review_digest {
            return Err(Error::Conflict);
        }
        if reviewed.action == ImportAction::Unchanged {
            let stored = read_import(&tx, plan.import_id())?.ok_or(Error::CorruptDatabase)?;
            tx.commit()?;
            return Ok(stored);
        }
        let project = plan.project_id();
        let name = plan
            .share()
            .name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&plan.share().project_id)
            .trim();
        let mapped: Option<String> = tx.query_row(
            "SELECT project_id FROM collaboration_project WHERE base_url=?1 AND remote_project_id=?2",
            params![plan.source().base_url(), plan.share().project_id], |r| r.get(0),
        ).optional()?;
        if mapped.is_none() {
            if read_named(&tx, Kind::Project, project)?.is_some() {
                return Err(Error::Conflict);
            }
            tx.execute(
                "INSERT INTO global_project(id,name,revision) VALUES(?1,?2,1)",
                params![project.to_string(), name],
            )?;
            tx.execute(
                "INSERT INTO collaboration_project VALUES(?1,?2,?3)",
                params![
                    plan.source().base_url(),
                    plan.share().project_id,
                    project.to_string()
                ],
            )?;
        } else if mapped.as_deref() != Some(&project.to_string()) {
            return Err(Error::Conflict);
        }
        // Create ordinary draft objectives once. Subsequent imports never
        // overwrite the operator's framing, exposures, priorities or grades.
        if super::plan::read_plan(&tx, project)?.is_none() {
            let objectives = draft_objectives(plan, project);
            let draft = super::plan::PlanDraft {
                project_id: project,
                revision: 1,
                objectives,
                contributions: vec![],
                updated_at_ms: now_ms,
            };
            super::plan::validate_plan(&draft)?;
            tx.execute(
                "INSERT INTO plan_draft(project_id,revision,payload) VALUES(?1,1,?2)",
                params![project.to_string(), super::configuration::encode(&draft)?],
            )?;
        }
        if super::framing::read_draft(&tx, project)?.is_none() {
            let region = &plan.share().region;
            let mas = psf_guard_director_core::program::MAS_PER_DEGREE as f64;
            let size = |region: &psf_guard_director_interop::astrocollab::Region| {
                psf_guard_director_core::framing::PanelSize {
                    width_degrees: f64::from(region.width_mas) / mas,
                    height_degrees: f64::from(region.height_mas) / mas,
                }
            };
            // A collaboration's whole sky region can exceed a camera panel.
            // Keep import independent of the framing editor's panel bounds.
            let panel = std::iter::once(region)
                .chain(plan.share().cells.iter().map(|c| &c.region))
                .map(size)
                .find(|panel| panel.validate().is_ok());
            let framing = super::framing::FramingDraft {
                project_id: project,
                revision: 1,
                target_name: name.into(),
                center: psf_guard_director_core::visibility::IcrsPosition {
                    ra_degrees: f64::from(region.icrs_ra_mas) / mas,
                    dec_degrees: f64::from(region.icrs_dec_mas) / mas,
                },
                position_angle_degrees: f64::from(region.position_angle_mas) / mas,
                mosaic: psf_guard_director_core::framing::Mosaic::SINGLE,
                panel_rig_id: Some(rig),
                panel,
                shown_rig_ids: vec![],
                survey_id: "dss2_color".into(),
                view_fov_degrees: (f64::from(region.width_mas.max(region.height_mas)) / mas * 1.2)
                    .clamp(0.02, 180.0),
                updated_at_ms: now_ms,
                rig_framings: vec![],
                layout_revision: 1,
            };
            super::framing::validate_draft(&framing)?;
            tx.execute(
                "INSERT INTO framing_draft(project_id,revision,payload) VALUES(?1,1,?2)",
                params![project.to_string(), super::configuration::encode(&framing)?],
            )?;
        }
        let revision = reviewed
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        tx.execute(
            "INSERT INTO collaboration_import VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(id) DO UPDATE SET digest=excluded.digest,payload=excluded.payload,
                 revision=excluded.revision,imported_at_ms=excluded.imported_at_ms",
            params![
                plan.import_id().to_string(),
                rig.to_string(),
                project.to_string(),
                plan.source().base_url(),
                plan.source().agent_id(),
                plan.night(),
                plan.share().task_id,
                plan.digest(),
                plan.snapshot(),
                revision as i64,
                now_ms as i64
            ],
        )?;
        tx.execute(
            "INSERT INTO collaboration_revision VALUES(?1,?2,?3,?4,?5)",
            params![
                plan.import_id().to_string(),
                plan.digest(),
                plan.snapshot(),
                revision as i64,
                now_ms as i64
            ],
        )?;
        let result = Imported {
            rig_id: rig,
            revision,
            imported_at_ms: now_ms,
            plan: plan.clone(),
        };
        tx.commit()?;
        Ok(result)
    }

    /// Imported source constraints and exact panels need admission support
    /// before they can become an active program. Editing a draft is not consent.
    pub fn collaboration_requires_admission(&self, project: Uuid) -> Result<bool, Error> {
        valid_id(project)?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_project WHERE project_id=?1)",
            [project.to_string()],
            |r| r.get(0),
        )?)
    }
}

fn draft_objectives(plan: &PreparedImport, project: Uuid) -> Vec<super::plan::Objective> {
    let filters: std::collections::BTreeMap<_, _> = plan
        .share()
        .demands
        .iter()
        .map(|d| (d.filter.as_str(), d.requested_frames))
        .collect();
    filters
        .into_iter()
        .map(|(filter, frames)| {
            // Do not feed OSC/custom labels through the core's mono aliases.
            let bandpass = match filter {
                "L" => "luminance",
                "R" => "red",
                "G" => "green",
                "B" => "blue",
                "H" => "h_alpha",
                "O" => "oiii",
                "S" => "sii",
                _ => "external_filter",
            };
            super::plan::Objective {
                id: Uuid::new_v5(&project, filter.as_bytes()),
                bandpass_id: bandpass.into(),
                purpose: "collaboration_visit".into(),
                goal: super::plan::Goal::Frames { value: frames },
                priority: 1,
            }
        })
        .collect()
}

fn preview(conn: &Connection, plan: &PreparedImport, rig: Uuid) -> Result<ImportPreview, Error> {
    valid_id(rig)?;
    if read_named(conn, Kind::Rig, rig)?.is_none() {
        return Err(Error::NotFound);
    }
    let wrong_rig: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM collaboration_import WHERE base_url=?1 AND agent_id=?2 AND rig_id!=?3
            UNION ALL SELECT 1 FROM collaboration_connection WHERE base_url=?1 AND agent_id=?2 AND rig_id!=?3)",
        params![plan.source().base_url(), plan.source().agent_id(), rig.to_string()], |r| r.get(0))?;
    if wrong_rig {
        return Err(Error::Conflict);
    }
    let existing = read_import(conn, plan.import_id())?;
    if let Some(old) = &existing {
        if old.rig_id != rig
            || old.plan.share().version > plan.share().version
            || (old.plan.share().version == plan.share().version
                && old.plan.digest() != plan.digest())
        {
            return Err(Error::Conflict);
        }
        let has_reports: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_outbox WHERE import_id=?1)",
            [plan.import_id().to_string()],
            |r| r.get(0),
        )?;
        if has_reports && old.plan.share().geometry_digest != plan.share().geometry_digest {
            return Err(Error::Conflict);
        }
    }
    let current = existing.as_ref().map_or(0, |i| i.revision);
    let local_plan = super::plan::read_plan(conn, plan.project_id())?;
    let framing = super::framing::read_draft(conn, plan.project_id())?;
    let key = serde_json::to_vec(&(
        plan.digest(),
        rig,
        current,
        existing.as_ref().map(|i| i.plan.digest()),
        local_plan.map(|p| p.revision),
        framing.map(|f| f.revision),
    ))
    .map_err(|_| Error::InvalidInput)?;
    Ok(ImportPreview {
        import_id: plan.import_id(),
        project_id: plan.project_id(),
        rig_id: rig,
        action: match existing {
            None => ImportAction::Create,
            Some(i) if i.plan.digest() == plan.digest() => ImportAction::Unchanged,
            Some(_) => ImportAction::Update,
        },
        expected_revision: current,
        review_digest: digest(&key),
        acquisition_enabled: false,
    })
}

fn read_import(conn: &Connection, id: Uuid) -> Result<Option<Imported>, Error> {
    valid_id(id)?;
    type Row = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Vec<u8>,
        i64,
        i64,
    );
    let row: Option<Row> = conn.query_row(
        "SELECT rig_id,project_id,base_url,agent_id,night,task_id,digest,substr(payload,1,?2),revision,imported_at_ms FROM collaboration_import WHERE id=?1",
        params![id.to_string(), (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?)),
    ).optional()?;
    row.map(
        |(rig, project, url, agent, night, task, hash, bytes, revision, at)| {
            let source = Source::new(&url, &agent, true).map_err(|_| Error::CorruptDatabase)?;
            let plan = prepare_import(&bytes, &source, &night, &task)
                .map_err(|_| Error::CorruptDatabase)?;
            if plan.import_id() != id
                || plan.project_id() != parse_id(&project)?
                || plan.digest() != hash
                || revision <= 0
                || at < 0
                || at as u64 > MAX_TIME_MS
            {
                return Err(Error::CorruptDatabase);
            }
            Ok(Imported {
                rig_id: parse_id(&rig)?,
                revision: revision as u64,
                imported_at_ms: at as u64,
                plan,
            })
        },
    )
    .transpose()
}

fn read_import_revision(
    conn: &Connection,
    id: Uuid,
    hash: &str,
) -> Result<Option<Imported>, Error> {
    valid_id(id)?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidInput);
    }
    let Some(owner) = read_import(conn, id)? else {
        return Ok(None);
    };
    let row: Option<(Vec<u8>, i64, i64)> = conn.query_row(
        "SELECT substr(payload,1,?3),revision,imported_at_ms FROM collaboration_revision WHERE import_id=?1 AND digest=?2",
        params![id.to_string(), hash, (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    row.map(|(bytes, revision, at)| {
        let plan = prepare_import(
            &bytes,
            owner.plan.source(),
            owner.plan.night(),
            &owner.plan.share().task_id,
        )
        .map_err(|_| Error::CorruptDatabase)?;
        if plan.import_id() != id
            || plan.project_id() != owner.plan.project_id()
            || plan.digest() != hash
            || revision <= 0
            || revision as u64 > owner.revision
            || at < 0
            || at as u64 > MAX_TIME_MS
        {
            return Err(Error::CorruptDatabase);
        }
        Ok(Imported {
            rig_id: owner.rig_id,
            revision: revision as u64,
            imported_at_ms: at as u64,
            plan,
        })
    })
    .transpose()
}

#[derive(Clone, Debug, Serialize)]
pub struct QueuedReport {
    pub id: Uuid,
    pub import_id: Uuid,
    pub panel_index: u32,
    pub filter: String,
    pub integration_ms: u64,
    pub payload: serde_json::Value,
    pub captures: Vec<Uuid>,
    pub images: Vec<Uuid>,
    pub created_at_ms: u64,
    pub recorded: Option<Recorded>,
    pub acknowledged_at_ms: Option<u64>,
}

impl MetaStore {
    /// Queue an immutable finalized snapshot. The same frames may extend their
    /// own aggregate, but may never be attributed to another panel or share.
    pub fn queue_collaboration_report(
        &mut self,
        finalized: &FinalizedContribution,
        now_ms: u64,
    ) -> Result<QueuedReport, Error> {
        valid_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        read_import(&tx, finalized.import_id())?.ok_or(Error::NotFound)?;
        let import = read_import_revision(&tx, finalized.import_id(), finalized.source_digest())?
            .ok_or(Error::NotFound)?;
        if now_ms < import.imported_at_ms {
            return Err(Error::InvalidInput);
        }
        if import.plan.share().geometry_digest != finalized.geometry_digest()
            || finalized.integration_ms() > i64::MAX as u64
        {
            return Err(Error::Conflict);
        }
        let payload = super::configuration::encode(finalized.report())?;
        let captures = super::configuration::encode(&finalized.captures())?;
        let images = super::configuration::encode(&finalized.images())?;
        let hash = report_digest(
            finalized.import_id(),
            finalized.source_digest(),
            finalized.geometry_digest(),
            &payload,
            &captures,
            &images,
        )?;
        let id = Uuid::new_v5(&finalized.import_id(), hash.as_bytes());
        if let Some(old) = read_report(&tx, id)? {
            tx.commit()?;
            return Ok(old);
        }
        // The wire key omits our import/revision IDs. Two nightly snapshots of
        // the same task must not silently replace one another at the server.
        // Hold this case until their measured cohorts can be merged losslessly.
        let competing: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_outbox o JOIN collaboration_import i ON i.id=o.import_id
            WHERE i.base_url=?1 AND i.agent_id=?2 AND i.task_id=?3 AND o.import_id!=?4 AND o.panel=?5 AND o.filter=?6 AND json_extract(o.payload,'$.night')=?7)",
            params![import.plan.source().base_url(),import.plan.source().agent_id(),import.plan.share().task_id,finalized.import_id().to_string(),finalized.panel_index(),finalized.filter(),finalized.observing_night()],|r|r.get(0))?;
        if competing {
            return Err(Error::Conflict);
        }
        let mut stmt = tx.prepare("SELECT id FROM collaboration_outbox WHERE import_id=?1 AND panel=?2 AND filter=?3 AND json_extract(payload,'$.night')=?4 ORDER BY integration_ms DESC LIMIT 1")?;
        let previous: Option<String> = stmt
            .query_row(
                params![
                    finalized.import_id().to_string(),
                    finalized.panel_index(),
                    finalized.filter(),
                    finalized.observing_night()
                ],
                |r| r.get(0),
            )
            .optional()?;
        drop(stmt);
        // A retry may extend one observing night's aggregate, never move the
        // same saved frame into another night's report and count it twice.
        let wrong_night: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_outbox o WHERE o.import_id=?1 AND json_extract(o.payload,'$.night')!=?2
            AND EXISTS(SELECT 1 FROM json_each(o.images) old JOIN json_each(?3) new ON old.value=new.value))",
            params![finalized.import_id().to_string(),finalized.observing_night(),images], |r| r.get(0))?;
        if wrong_night {
            return Err(Error::Conflict);
        }
        if let Some(old) = previous {
            let old = read_report(&tx, parse_id(&old)?)?.ok_or(Error::CorruptDatabase)?;
            let captures: BTreeSet<_> = finalized.captures().iter().copied().collect();
            let images: BTreeSet<_> = finalized.images().iter().copied().collect();
            if finalized.integration_ms() <= old.integration_ms
                || old.captures.iter().any(|id| !captures.contains(id))
                || old.images.iter().any(|id| !images.contains(id))
            {
                return Err(Error::Conflict);
            }
        }
        for (capture, image) in finalized.captures().iter().zip(finalized.images()) {
            let old: Option<(String, String, String, i64, String)> = tx.query_row(
                "SELECT capture_id,image_guid,import_id,panel,filter FROM collaboration_capture WHERE capture_id=?1 OR image_guid=?2",
                params![capture.to_string(), image.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            if let Some((old_capture, old_image, old_import, panel, filter)) = old {
                if old_capture != capture.to_string()
                    || old_image != image.to_string()
                    || old_import != finalized.import_id().to_string()
                    || panel != i64::from(finalized.panel_index())
                    || filter != finalized.filter()
                {
                    return Err(Error::Conflict);
                }
            } else {
                tx.execute(
                    "INSERT INTO collaboration_capture VALUES(?1,?2,?3,?4,?5)",
                    params![
                        capture.to_string(),
                        image.to_string(),
                        finalized.import_id().to_string(),
                        finalized.panel_index(),
                        finalized.filter()
                    ],
                )?;
            }
        }
        tx.execute("INSERT INTO collaboration_outbox(id,import_id,panel,filter,integration_ms,source_digest,geometry_digest,payload,captures,images,digest,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![id.to_string(),finalized.import_id().to_string(),finalized.panel_index(),finalized.filter(),finalized.integration_ms() as i64,
                finalized.source_digest(),finalized.geometry_digest(),payload,captures,images,hash,now_ms as i64])?;
        let record = read_report(&tx, id)?.ok_or(Error::CorruptDatabase)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn collaboration_report(&self, id: Uuid) -> Result<Option<QueuedReport>, Error> {
        read_report(&self.connection, id)
    }

    pub fn latest_collaboration_report(
        &self,
        import: Uuid,
        panel: u32,
        filter: &str,
        night: &str,
    ) -> Result<Option<QueuedReport>, Error> {
        let id: Option<String> = self.connection.query_row(
            "SELECT id FROM collaboration_outbox WHERE import_id=?1 AND panel=?2 AND filter=?3 AND json_extract(payload,'$.night')=?4 ORDER BY integration_ms DESC LIMIT 1",
            params![import.to_string(), panel, filter, night], |r| r.get(0)).optional()?;
        id.map(|id| read_report(&self.connection, parse_id(&id)?)?.ok_or(Error::CorruptDatabase))
            .transpose()
    }

    /// One oldest unacknowledged snapshot per report key. New frames queued
    /// while a batch is in flight are not part of its acknowledgement.
    pub fn pending_collaboration_reports(
        &self,
        source: &Source,
        limit: usize,
    ) -> Result<Vec<QueuedReport>, Error> {
        if !(1..=MAX_REPORTS).contains(&limit) {
            return Err(Error::InvalidInput);
        }
        let mut stmt = self.connection.prepare(
            "SELECT o.id FROM collaboration_outbox o JOIN collaboration_import i ON i.id=o.import_id
             WHERE i.base_url=?1 AND i.agent_id=?2 AND o.acknowledged_at_ms IS NULL
             AND NOT EXISTS(SELECT 1 FROM collaboration_outbox p WHERE p.import_id=o.import_id AND p.panel=o.panel AND p.filter=o.filter
                AND json_extract(p.payload,'$.night')=json_extract(o.payload,'$.night')
                AND p.acknowledged_at_ms IS NULL AND p.integration_ms<o.integration_ms)
             ORDER BY o.created_at_ms,o.id LIMIT ?3")?;
        let ids = stmt
            .query_map(
                params![source.base_url(), source.agent_id(), limit as i64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| read_report(&self.connection, parse_id(&id)?)?.ok_or(Error::CorruptDatabase))
            .collect()
    }

    /// Validate the entire positional reply before acknowledging any row.
    /// Remote verdicts never change local image grades or execution progress.
    pub fn acknowledge_collaboration_reports(
        &mut self,
        source: &Source,
        ids: &[Uuid],
        reply: &[u8],
        now_ms: u64,
    ) -> Result<Vec<QueuedReport>, Error> {
        valid_time(now_ms)?;
        if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(Error::InvalidInput);
        }
        let recorded = decode_recorded(reply, ids.len()).map_err(|_| Error::InvalidInput)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut results = Vec::new();
        for (id, received) in ids.iter().zip(recorded) {
            let mut record = read_report(&tx, *id)?.ok_or(Error::NotFound)?;
            if now_ms < record.created_at_ms {
                return Err(Error::InvalidInput);
            }
            let import = read_import(&tx, record.import_id)?.ok_or(Error::CorruptDatabase)?;
            if import.plan.source() != source {
                return Err(Error::Conflict);
            }
            if let Some(old) = &record.recorded {
                // A retry may change duplicate/verdict metadata. Retain the
                // first receipt, but never accept a different remote identity.
                if old.id != received.id {
                    return Err(Error::Conflict);
                }
            } else {
                tx.execute("UPDATE collaboration_outbox SET recorded=?2,acknowledged_at_ms=?3 WHERE id=?1 AND acknowledged_at_ms IS NULL",
                    params![id.to_string(), super::configuration::encode(&received)?, now_ms as i64])?;
                record.recorded = Some(received);
                record.acknowledged_at_ms = Some(now_ms);
            }
            results.push(record);
        }
        tx.commit()?;
        Ok(results)
    }
}

fn report_digest(
    import: Uuid,
    source: &str,
    geometry: &str,
    payload: &str,
    captures: &str,
    images: &str,
) -> Result<String, Error> {
    Ok(digest(
        &serde_json::to_vec(&(import, source, geometry, payload, captures, images))
            .map_err(|_| Error::InvalidInput)?,
    ))
}

fn read_report(conn: &Connection, id: Uuid) -> Result<Option<QueuedReport>, Error> {
    valid_id(id)?;
    type Row = (
        String,
        i64,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<i64>,
    );
    let row: Option<Row> = conn.query_row(
        "SELECT import_id,panel,filter,integration_ms,source_digest,geometry_digest,substr(payload,1,262145),substr(captures,1,262145),substr(images,1,262145),digest,created_at_ms,substr(recorded,1,262145),acknowledged_at_ms FROM collaboration_outbox WHERE id=?1",
        [id.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?,r.get(12)?)),
    ).optional()?;
    row.map(
        |(
            import,
            panel,
            filter,
            total,
            source,
            geometry,
            payload,
            captures,
            images,
            hash,
            created,
            recorded,
            ack,
        )| {
            let import_id = parse_id(&import)?;
            let owner = read_import(conn, import_id)?.ok_or(Error::CorruptDatabase)?;
            let original =
                read_import_revision(conn, import_id, &source)?.ok_or(Error::CorruptDatabase)?;
            let captures_v: Vec<Uuid> = super::configuration::decode(captures.as_bytes().to_vec())?;
            let images_v: Vec<Uuid> = super::configuration::decode(images.as_bytes().to_vec())?;
            let body: serde_json::Value =
                super::configuration::decode(payload.as_bytes().to_vec())?;
            let demand = original
                .plan
                .share()
                .demands
                .iter()
                .find(|d| i64::from(d.panel_index) == panel && d.filter == filter);
            if report_digest(import_id, &source, &geometry, &payload, &captures, &images)? != hash
                || Uuid::new_v5(&import_id, hash.as_bytes()) != id
                || panel < 0
                || panel > u32::MAX as i64
                || total <= 0
                || created < 0
                || created as u64 > MAX_TIME_MS
                || ack.is_some_and(|n| n < created || n as u64 > MAX_TIME_MS)
                || ack.is_some() != recorded.is_some()
                || geometry != original.plan.share().geometry_digest
                || captures_v.is_empty()
                || captures_v.len() > MAX_REPORT_FRAMES
                || captures_v.len() != images_v.len()
                || captures_v.iter().any(Uuid::is_nil)
                || images_v.iter().any(Uuid::is_nil)
                || captures_v.iter().collect::<BTreeSet<_>>().len() != captures_v.len()
                || images_v.iter().collect::<BTreeSet<_>>().len() != images_v.len()
                || body["project"].as_str() != Some(&owner.plan.share().project_id)
                || body["task"].as_str() != Some(&owner.plan.share().task_id)
                || body["night"].as_str().is_none_or(|night| {
                    psf_guard_director_interop::astrocollab::validate_night(night).is_err()
                })
                || body["filterName"].as_str() != Some(&filter)
                || body["panel"].as_str() != Some(&panel.to_string())
                || body["frames"].as_u64() != Some(captures_v.len() as u64)
                || body["seconds"].as_f64() != Some(total as f64 / 1000.0)
                || !demand.is_some_and(|d| {
                    let count = captures_v.len() as u64;
                    let measured = total as u64;
                    exposure_matches(d.exposure_ms, measured / count)
                        && exposure_matches(d.exposure_ms, measured.div_ceil(count))
                        && body["exposure"].as_f64() == Some(total as f64 / count as f64 / 1000.0)
                })
            {
                return Err(Error::CorruptDatabase);
            }
            let recorded: Option<Recorded> = recorded
                .map(|s| super::configuration::decode(s.into_bytes()))
                .transpose()?;
            if let Some(receipt) = &recorded {
                let bytes = serde_json::to_vec(&serde_json::json!({"recorded":[receipt]}))
                    .map_err(|_| Error::CorruptDatabase)?;
                decode_recorded(&bytes, 1).map_err(|_| Error::CorruptDatabase)?;
            }
            Ok(QueuedReport {
                id,
                import_id,
                panel_index: panel as u32,
                filter,
                integration_ms: total as u64,
                payload: body,
                captures: captures_v,
                images: images_v,
                created_at_ms: created as u64,
                recorded,
                acknowledged_at_ms: ack.map(|n| n as u64),
            })
        },
    )
    .transpose()
}
