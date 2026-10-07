//! Translate reviewed nightly work into ordinary activation rows. Credentials
//! and scientific reports stay on the coordinator, never in the TS database.
use super::*;
use psf_guard_director_core::framing::{FramingRequest, Mosaic, Panel, PanelSize};
use psf_guard_director_interop::{astrocollab, collaboration::PreparedImport};
use psf_guard_director_meta::{collaboration::Imported, plan::Contribution};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Visit {
    pub import_id: Uuid,
    pub source_digest: String,
}

pub(super) const TABLE: &str = "psf_guard_collaboration_plan";
pub(super) const DDL: &str = "CREATE TABLE IF NOT EXISTS psf_guard_collaboration_plan(
    exposureplan_guid TEXT NOT NULL,
    target_guid TEXT NOT NULL, import_id TEXT NOT NULL, source_digest TEXT NOT NULL,
    panel_index INTEGER NOT NULL, filter TEXT NOT NULL, exposure_ms INTEGER NOT NULL,
    start_at_ms INTEGER NOT NULL, end_at_ms INTEGER,
    desired_frames INTEGER NOT NULL,
    PRIMARY KEY(exposureplan_guid,import_id,source_digest));
    CREATE INDEX IF NOT EXISTS psf_guard_collaboration_target ON psf_guard_collaboration_plan(target_guid);";

pub(super) async fn context(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<serde_json::Value>>, Error> {
    let result = enabled(&state)?
        .query(move |s| {
            s.project(id)?.ok_or(StoreError::NotFound)?;
            let mut imports = Vec::new();
            let mut after = None;
            loop {
                let page = s.collaboration_import_ids(id, after, 256)?;
                for import_id in page.ids {
                    let i = s
                        .collaboration_import(import_id)?
                        .ok_or(StoreError::NotFound)?;
                    imports.push(serde_json::json!({
                        "import_id": import_id, "rig_id": i.rig_id,
                        "rig_name": s.rig(i.rig_id)?.map(|r| r.name),
                        "source_digest": i.plan.digest(), "night": i.plan.night(),
                        "task_id": i.plan.share().task_id, "version": i.plan.share().version,
                        "demands": i.plan.share().demands,
                    }));
                }
                match page.next_after {
                    Some(next) => after = Some(next),
                    None => break,
                }
                if imports.len() >= 1024 {
                    return Err(StoreError::InvalidInput);
                }
            }
            Ok(serde_json::json!({"imports": imports}))
        })
        .await?;
    Ok(Json(ApiResponse::success(result)))
}

fn not_ready(message: &'static str) -> activation::ActivationError {
    activation::ActivationError::NotReady(message)
}

/// Restore only this project's current assignment from its identity-bound rig
/// catalogs. This is a read-only status check, never admission of new work.
pub(super) fn current(
    store: &MetaStore,
    project: Uuid,
    instance: Uuid,
    catalogs: &[Arc<DatabaseContext>],
) -> Result<Vec<Visit>, activation::ActivationError> {
    let found = identified_catalogs(catalogs, instance);
    let mut visits = BTreeMap::new();
    for (identity, catalog) in found.iter() {
        let Some(binding) = store.catalog_rig(identity.id)? else {
            continue;
        };
        let conn = crate::server::database_context::open_scheduler_connection_with_flags(
            &catalog.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [TABLE],
            |r| r.get(0),
        )?;
        if !exists {
            continue;
        }
        let mut stmt = conn.prepare("SELECT DISTINCT c.import_id,c.source_digest FROM psf_guard_collaboration_plan c
            JOIN psf_guard_director_target t ON t.target_guid=c.target_guid
            JOIN psf_guard_director_project p ON p.project_guid=t.project_guid
            WHERE p.global_project_id=?1 AND p.coordinator_instance_id=?2 AND c.end_at_ms IS NULL LIMIT 33")?;
        let rows = stmt
            .query_map(params![project.to_string(), instance.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, source_digest) in rows {
            let import_id = Uuid::parse_str(&id).map_err(|_| Error::Invalid)?;
            let imported = store
                .collaboration_import(import_id)?
                .ok_or(Error::Missing)?;
            if imported.rig_id != binding.rig.id
                || visits
                    .insert(
                        binding.rig.id,
                        Visit {
                            import_id,
                            source_digest,
                        },
                    )
                    .is_some()
            {
                return Err(not_ready(
                    "Rig assignment identity is ambiguous; review Activation.",
                ));
            }
        }
    }
    Ok(visits.into_values().collect())
}

pub(super) struct Admitted {
    pub visit: Visit,
    pub import: Imported,
    pub panels: Vec<Panel>,
}

pub(super) fn admit(
    store: &MetaStore,
    project: Uuid,
    visits: &[Visit],
) -> Result<BTreeMap<Uuid, Admitted>, activation::ActivationError> {
    let mut result = BTreeMap::new();
    for visit in visits {
        let import = store
            .collaboration_import(visit.import_id)?
            .ok_or(Error::Missing)?;
        if import.plan.project_id() != project || import.plan.digest() != visit.source_digest {
            return Err(Error::Conflict.into());
        }
        let panels = panels(&import.plan).map_err(|_| Error::Invalid)?;
        if result
            .insert(
                import.rig_id,
                Admitted {
                    visit: visit.clone(),
                    import,
                    panels,
                },
            )
            .is_some()
        {
            return Err(not_ready(
                "Select exactly one nightly assignment per participating rig.",
            ));
        }
    }
    Ok(result)
}

fn panel_id(import: &PreparedImport, index: u32) -> String {
    Uuid::new_v5(
        &import.project_id(),
        format!("{}:{index}", import.share().geometry_digest).as_bytes(),
    )
    .simple()
    .to_string()
}

fn panels(import: &PreparedImport) -> Result<Vec<Panel>, ()> {
    let share = import.share();
    share
        .panel_order
        .iter()
        .map(|index| {
            let (region, row, column) = if share.kind == astrocollab::Kind::Single {
                (&share.region, 0, 0)
            } else {
                let cell = share.cells.iter().find(|c| c.index == *index).ok_or(())?;
                (&cell.region, cell.row, cell.column)
            };
            let mas = psf_guard_director_core::program::MAS_PER_DEGREE as f64;
            let mut panel = FramingRequest {
                center: psf_guard_director_core::visibility::IcrsPosition {
                    ra_degrees: f64::from(region.icrs_ra_mas) / mas,
                    dec_degrees: f64::from(region.icrs_dec_mas) / mas,
                },
                position_angle_degrees: f64::from(region.position_angle_mas) / mas,
                panel: PanelSize {
                    width_degrees: f64::from(region.width_mas) / mas,
                    height_degrees: f64::from(region.height_mas) / mas,
                },
                mosaic: Mosaic::SINGLE,
                overlays: vec![],
                view: None,
            }
            .preview()
            .map_err(|_| ())?
            .panels
            .remove(0);
            panel.row = row;
            panel.column = column;
            panel.footprint.id = panel_id(import, *index);
            Ok(panel)
        })
        .collect()
}

impl Admitted {
    pub fn plan_for(
        &self,
        tx: &Connection,
        target: &str,
        filter: &str,
    ) -> rusqlite::Result<Option<String>> {
        let filter = astrocollab::fold_filter(filter).map_err(|_| rusqlite::Error::InvalidQuery)?;
        tx.query_row("SELECT c.exposureplan_guid FROM psf_guard_collaboration_plan c JOIN exposureplan e ON e.guid=c.exposureplan_guid
            WHERE c.target_guid=?1 AND c.filter=?2 ORDER BY c.start_at_ms DESC,c.rowid DESC LIMIT 1",params![target,filter],|r|r.get(0)).optional()
    }

    pub fn panel_index(&self, panel: &str) -> Option<u32> {
        self.import
            .plan
            .share()
            .panel_order
            .iter()
            .copied()
            .find(|index| panel_id(&self.import.plan, *index) == panel)
    }

    pub fn prepare(
        &self,
        tx: &Connection,
        project: &str,
        now: u64,
    ) -> Result<bool, activation::RigError> {
        tx.execute_batch(DDL)?;
        let previous: Option<Option<i64>> = tx.query_row(
            "SELECT end_at_ms FROM psf_guard_collaboration_plan c JOIN psf_guard_director_target t ON t.target_guid=c.target_guid
             WHERE t.project_guid=?1 AND c.import_id=?2 AND c.source_digest=?3 LIMIT 1",
            params![project,self.visit.import_id.to_string(),self.visit.source_digest], |r| r.get(0)).optional()?;
        if previous.flatten().is_some() {
            return Err(activation::RigError::Skip("This assignment was superseded. Pull and review current work before activating it.".into()));
        }
        tx.execute("UPDATE psf_guard_collaboration_plan SET end_at_ms=?2 WHERE end_at_ms IS NULL
            AND target_guid IN (SELECT target_guid FROM psf_guard_director_target WHERE project_guid=?1)
            AND NOT (import_id=?3 AND source_digest=?4)",
            params![project,(now/1000*1000) as i64,self.visit.import_id.to_string(),self.visit.source_digest])?;
        Ok(previous.is_none())
    }

    pub fn frame_goal(
        &self,
        tx: &Connection,
        plan: Option<&str>,
        requested: u32,
    ) -> rusqlite::Result<u32> {
        let Some(plan) = plan else {
            return Ok(requested);
        };
        let previous: Option<u32> = tx.query_row(
            "SELECT desired_frames FROM psf_guard_collaboration_plan WHERE exposureplan_guid=?1 AND import_id=?2 AND source_digest=?3 AND end_at_ms IS NULL",
            params![plan,self.visit.import_id.to_string(),self.visit.source_digest], |r| r.get(0)).optional()?;
        if let Some(desired) = previous {
            return Ok(desired);
        }
        let graded = activation::has_column(tx, "project", "enablegrader")?
            && activation::has_column(tx, "exposureplan", "accepted")?;
        let sql = if graded {
            "SELECT CASE WHEN COALESCE(p.enablegrader,0)!=0 THEN COALESCE(e.accepted,0) ELSE COALESCE(e.acquired,0) END FROM exposureplan e JOIN target t ON t.Id=e.targetId JOIN project p ON p.Id=t.projectId WHERE e.guid=?1"
        } else {
            "SELECT COALESCE(acquired,0) FROM exposureplan WHERE guid=?1"
        };
        let baseline: Option<i64> = tx.query_row(sql, [plan], |r| r.get(0)).optional()?;
        baseline
            .unwrap_or(0)
            .checked_add(i64::from(requested))
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(rusqlite::Error::InvalidQuery)
    }

    pub fn rotation(&self, panel_id: &str) -> Option<f64> {
        let index = self
            .import
            .plan
            .share()
            .panel_order
            .iter()
            .find(|index| self::panel_id(&self.import.plan, **index) == panel_id)?;
        let s = self.import.plan.share();
        let region = if s.kind == astrocollab::Kind::Single {
            &s.region
        } else {
            &s.cells.iter().find(|c| c.index == *index)?.region
        };
        Some(
            f64::from(region.position_angle_mas)
                / psf_guard_director_core::program::MAS_PER_DEGREE as f64,
        )
    }

    pub fn validate_recipes(
        &self,
        contributions: &[&Contribution],
    ) -> Result<(), activation::ActivationError> {
        let mut matched = BTreeSet::new();
        for c in contributions {
            let filter =
                astrocollab::fold_filter(&c.template.filter_name).map_err(|_| Error::Invalid)?;
            let demands: Vec<_> = self
                .import
                .plan
                .share()
                .demands
                .iter()
                .filter(|d| d.filter == filter)
                .collect();
            if demands.is_empty()
                || !matched.insert(filter.clone())
                || !c.panel_ids.is_empty()
                || (c.exposure_seconds * 1000.0 - demands[0].exposure_ms as f64).abs() > 0.001
            {
                return Err(not_ready("Use one enabled recipe per assigned filter, its assigned exposure, and all assigned panels."));
            }
        }
        let filters: BTreeSet<_> = self
            .import
            .plan
            .share()
            .demands
            .iter()
            .map(|d| d.filter.clone())
            .collect();
        if matched != filters {
            return Err(not_ready(
                "Add an enabled recipe for every filter in this nightly assignment.",
            ));
        }
        Ok(())
    }

    pub fn frames(&self, panel: &str, filter: &str) -> Option<(u32, u32)> {
        let filter = astrocollab::fold_filter(filter).ok()?;
        self.import
            .plan
            .share()
            .demands
            .iter()
            .find(|d| panel_id(&self.import.plan, d.panel_index) == panel && d.filter == filter)
            .map(|d| (d.panel_index, d.requested_frames))
    }

    pub fn record(
        &self,
        tx: &Connection,
        plan: &str,
        target: &str,
        panel: &str,
        c: &Contribution,
        now: u64,
    ) -> rusqlite::Result<u32> {
        tx.execute_batch(DDL)?;
        let (index, requested) = self
            .frames(panel, &c.template.filter_name)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let previous: Option<(u32, Option<i64>)> = tx.query_row(
            "SELECT desired_frames,end_at_ms FROM psf_guard_collaboration_plan WHERE exposureplan_guid=?1 AND import_id=?2 AND source_digest=?3",
            params![plan,self.visit.import_id.to_string(),self.visit.source_digest], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((desired, None)) = previous {
            tx.execute(
                "UPDATE exposureplan SET desired=?2 WHERE guid=?1",
                params![plan, desired],
            )?;
            return Ok(desired);
        }
        if previous.is_some() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let desired = self.frame_goal(tx, Some(plan), requested)?;
        let now = (now / 1000 * 1000) as i64;
        tx.execute("UPDATE psf_guard_collaboration_plan SET end_at_ms=?2 WHERE target_guid=?1 AND filter=?3 AND end_at_ms IS NULL",
            params![target,now,astrocollab::fold_filter(&c.template.filter_name).map_err(|_|rusqlite::Error::InvalidQuery)?])?;
        tx.execute(
            "INSERT INTO psf_guard_collaboration_plan VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9)",
            params![
                plan,
                target,
                self.import.plan.import_id().to_string(),
                self.import.plan.digest(),
                index,
                astrocollab::fold_filter(&c.template.filter_name)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                (c.exposure_seconds * 1000.0).round() as i64,
                now,
                desired
            ],
        )?;
        tx.execute(
            "UPDATE exposureplan SET desired=?2 WHERE guid=?1",
            params![plan, desired],
        )?;
        Ok(desired)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Association {
    pub target_guid: String,
    pub source_digest: String,
    pub panel: u32,
    pub filter: String,
    pub exposure_ms: u64,
    pub start_at_ms: u64,
    pub end_at_ms: Option<u64>,
}

pub(super) fn associations(conn: &Connection, import: Uuid) -> rusqlite::Result<Vec<Association>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [TABLE],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare("SELECT target_guid,source_digest,panel_index,filter,exposure_ms,start_at_ms,end_at_ms FROM psf_guard_collaboration_plan WHERE import_id=?1 LIMIT 4097")?;
    let result: Vec<_> = stmt
        .query_map([import.to_string()], |r| {
            Ok(Association {
                target_guid: r.get(0)?,
                source_digest: r.get(1)?,
                panel: r.get(2)?,
                filter: r.get(3)?,
                exposure_ms: unsigned(r, 4)?,
                start_at_ms: unsigned(r, 5)?,
                end_at_ms: r
                    .get::<_, Option<i64>>(6)?
                    .map(|n| u64::try_from(n).map_err(|_| rusqlite::Error::InvalidQuery))
                    .transpose()?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if result.len() > 4096 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(result)
}

fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(index)?).map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(super) fn matching<'a>(
    associations: &'a [Association],
    target: &str,
    filter: &str,
    time: i64,
) -> Vec<&'a Association> {
    let Ok(filter) = astrocollab::fold_filter(filter) else {
        return vec![];
    };
    let Some(time) = time.checked_mul(1000).and_then(|t| u64::try_from(t).ok()) else {
        return vec![];
    };
    associations
        .iter()
        .filter(|a| {
            a.target_guid.eq_ignore_ascii_case(target)
                && a.filter == filter
                && a.start_at_ms <= time
                && a.end_at_ms.is_none_or(|end| time < end)
        })
        .collect()
}
