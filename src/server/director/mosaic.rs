//! The mosaic preview: each activated panel's latest stack preview, placed
//! by its plate solve over the plan's geometry. A review aid for coverage and
//! seams, never a processed image; panels from different rigs stay apart.

use super::*;
use crate::server::{
    database_context::DatabaseContext,
    sky_coverage::{previews_for, SkyPreview},
};
use rusqlite::{OpenFlags, OptionalExtension};
use std::{collections::BTreeMap, time::Duration};

#[derive(Serialize)]
struct PanelProgress {
    desired: i64,
    acquired: i64,
    accepted: i64,
}

#[derive(Serialize)]
struct MosaicPanel {
    panel_id: String,
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: String,
    target_guid: Uuid,
    /// The target's row and name in the rig database, when it still exists.
    target_id: Option<i64>,
    target_name: Option<String>,
    /// Frames the panel's exposure plans ask for, have, and have accepted.
    progress: Option<PanelProgress>,
    /// `ready`: a stack preview placed by its plate solve. `unsolved`: a stack
    /// exists but no solve reaches its grid, so it cannot be placed.
    /// `no_stack`, `missing_target`, `missing_catalog`: nothing to draw yet.
    status: &'static str,
    preview: Option<SkyPreview>,
}

#[derive(Serialize)]
pub(super) struct Mosaic {
    project: NamedIdentity,
    /// `None` until the project has been activated; then nothing has targets.
    activation_revision: Option<u64>,
    framing_revision: Option<u64>,
    /// The saved framing moved on since the activation. Solved stacks still
    /// sit where their solves put them; the plan's panels may have moved.
    framing_stale: bool,
    panels: Vec<MosaicPanel>,
    warnings: Vec<String>,
}

fn target_row(
    connection: &rusqlite::Connection,
    guid: Uuid,
) -> rusqlite::Result<Option<(i64, String)>> {
    connection
        .query_row(
            "SELECT Id, name FROM target WHERE guid = ?1",
            [guid.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

fn progress(connection: &rusqlite::Connection, target_id: i64) -> rusqlite::Result<PanelProgress> {
    connection.query_row(
        "SELECT COALESCE(SUM(desired), 0), COALESCE(SUM(acquired), 0), COALESCE(SUM(accepted), 0)
         FROM exposureplan WHERE targetid = ?1",
        [target_id],
        |row| {
            Ok(PanelProgress {
                desired: row.get(0)?,
                acquired: row.get(1)?,
                accepted: row.get(2)?,
            })
        },
    )
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Mosaic>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let metadata_permit = admit(&service.admission).await?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let mosaic = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let store = service.store.lock().map_err(|_| Error::Internal)?;
        let project = store.project(id)?.ok_or(Error::Missing)?;
        let framing_revision = store.framing_draft(id)?.map(|draft| draft.revision);
        let Some(activation) = store.activation(id)? else {
            return Ok(Mosaic {
                project,
                activation_revision: None,
                framing_revision,
                framing_stale: false,
                panels: vec![],
                warnings: vec![
                    "Activate the plan first; stacks appear once the rigs have shot their panels."
                        .into(),
                ],
            });
        };
        let framing_stale =
            framing_revision.is_some_and(|revision| revision != activation.framing_revision);
        // One preview index read per rig database, however many panels it owns.
        let mut previews: BTreeMap<String, std::collections::HashMap<i32, SkyPreview>> =
            BTreeMap::new();
        let mut panels = Vec::new();
        let mut warnings = Vec::new();
        let found = identified_catalogs(&catalogs, service.instance_id);
        warnings.extend(found.duplicates.iter().cloned());
        for activated in &activation.rigs {
            let rig = store.rig(activated.rig_id)?.ok_or(Error::Missing)?;
            let Some(context) = found.get(activated.catalog_id) else {
                warnings.push(format!(
                    "{}: its database is no longer registered on this server.",
                    rig.name
                ));
                for target in &activated.targets {
                    panels.push(MosaicPanel {
                        panel_id: target.panel_id.clone(),
                        rig: rig.clone(),
                        catalog_slug: None,
                        catalog_name: String::new(),
                        target_guid: target.target_guid,
                        target_id: None,
                        target_name: None,
                        progress: None,
                        status: "missing_catalog",
                        preview: None,
                    });
                }
                continue;
            };
            let connection = super::super::database_context::open_scheduler_connection_with_flags(
                FilePath::new(&context.database_path),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(StoreError::from)?;
            connection
                .busy_timeout(Duration::from_secs(2))
                .map_err(StoreError::from)?;
            let found = previews
                .entry(context.id.clone())
                .or_insert_with(|| previews_for(&context.cache_dir_path));
            for target in &activated.targets {
                let row = target_row(&connection, target.target_guid).map_err(StoreError::from)?;
                let (target_id, target_name) = match row {
                    Some((target_id, name)) => (Some(target_id), Some(name)),
                    None => (None, None),
                };
                let progress = target_id
                    .map(|target_id| progress(&connection, target_id))
                    .transpose()
                    .map_err(StoreError::from)?;
                let preview = target_id
                    .and_then(|target_id| i32::try_from(target_id).ok())
                    .and_then(|target_id| found.get(&target_id).cloned());
                let status = match (&target_id, &preview) {
                    (None, _) => "missing_target",
                    (Some(_), None) => "no_stack",
                    (Some(_), Some(preview)) if preview.wcs.is_some() => "ready",
                    (Some(_), Some(_)) => "unsolved",
                };
                panels.push(MosaicPanel {
                    panel_id: target.panel_id.clone(),
                    rig: rig.clone(),
                    catalog_slug: Some(context.id.clone()),
                    catalog_name: context.name.clone(),
                    target_guid: target.target_guid,
                    target_id,
                    target_name,
                    progress,
                    status,
                    preview,
                });
            }
        }
        panels.sort_by(|a, b| {
            a.panel_id
                .cmp(&b.panel_id)
                .then_with(|| a.rig.name.cmp(&b.rig.name))
        });
        Ok::<_, Error>(Mosaic {
            project,
            activation_revision: Some(activation.revision),
            framing_revision,
            framing_stale,
            panels,
            warnings,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director mosaic worker failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(mosaic)))
}
