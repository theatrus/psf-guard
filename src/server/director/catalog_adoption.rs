//! Explicit operator preview/apply; never a startup or source-sync side effect.
use super::*;
use crate::catalog_identity;
use psf_guard_director_meta::{catalog::ProjectMapping, CatalogIdentity};
use rusqlite::{OpenFlags, TransactionBehavior};
use std::time::Duration;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    catalog_id: Uuid,
    mappings: Vec<ProjectMapping>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Apply {
    plan: Plan,
    preview_digest: String,
}

#[derive(Serialize)]
struct ReviewedMapping {
    mapping: ProjectMapping,
    source_name: String,
    project: NamedIdentity,
    rig: NamedIdentity,
}

#[derive(Serialize)]
pub(super) struct Report {
    catalog_identity: CatalogIdentity,
    preview_digest: String,
    mappings: Vec<ReviewedMapping>,
    applied: bool,
}

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(plan): Json<Plan>,
) -> Result<Json<ApiResponse<Report>>, AdoptionError> {
    execute(state, slug, plan, None).await
}

pub(super) async fn apply(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(request): Json<Apply>,
) -> Result<Json<ApiResponse<Report>>, AdoptionError> {
    if request.preview_digest.len() != 64
        || !request
            .preview_digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(Error::Invalid.into());
    }
    execute(state, slug, request.plan, Some(request.preview_digest)).await
}

async fn execute(
    state: Arc<AppState>,
    slug: String,
    plan: Plan,
    expected: Option<String>,
) -> Result<Json<ApiResponse<Report>>, AdoptionError> {
    let service = enabled(&state)?;
    let mut projects = std::collections::HashSet::new();
    if plan.catalog_id.is_nil()
        || !(1..=256).contains(&plan.mappings.len())
        || plan
            .mappings
            .iter()
            .any(|mapping| mapping.catalog_id != plan.catalog_id)
        || plan
            .mappings
            .iter()
            .any(|mapping| !projects.insert(mapping.source_project_guid))
    {
        return Err(Error::Invalid.into());
    }
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    // Both permits stay with the worker on HTTP cancellation. Only this workflow
    // needs both; other metadata and discovery operations use one each.
    let metadata_permit = service
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let catalog_permit = service
        .discovery_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let report = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let applying = expected.is_some();
        let flags = if applying {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
            FilePath::new(&catalog.database_path),
            flags | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(StoreError::from)
        .map_err(Error::from)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(StoreError::from)
            .map_err(Error::from)?;
        let mut tx = connection
            .transaction_with_behavior(if applying {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })
            .map_err(StoreError::from)
            .map_err(Error::from)?;
        let evidence = catalog_discovery::read_evidence(&tx)?;
        let source_names = evidence.mapping_names(&plan.mappings)?;
        let saved_identity = catalog_identity::read(&tx)?;
        let require_new = saved_identity.is_none();
        let identity = saved_identity.unwrap_or(CatalogIdentity {
            id: plan.catalog_id,
            origin_instance_id: service.instance_id,
        });
        if identity.id != plan.catalog_id {
            return Err(Error::Conflict.into());
        }

        // Finish discovery before taking the shared metadata mutex.
        let mut store = service.store.lock().map_err(|_| Error::Internal)?;
        store
            .preview_catalog_adoption(identity, &plan.mappings, require_new)
            .map_err(Error::from)?;
        let mappings = plan
            .mappings
            .iter()
            .zip(source_names)
            .map(|(mapping, source_name)| {
                Ok(ReviewedMapping {
                    mapping: mapping.clone(),
                    source_name,
                    project: store
                        .project(mapping.project_id)
                        .map_err(Error::from)?
                        .ok_or(Error::Missing)?,
                    rig: store
                        .rig(mapping.rig_id)
                        .map_err(Error::from)?
                        .ok_or(Error::Missing)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        // A preview binds the exact evidence, choices, names/revisions, instance,
        // and registered locator. The locator is hashed, never returned as identity.
        let bytes = serde_json::to_vec(&(
            &plan,
            identity,
            &evidence,
            &mappings,
            service.instance_id,
            &catalog.id,
            &catalog.name,
            &catalog.database_path,
        ))
        .map_err(|_| Error::Internal)?;
        let digest = catalog_discovery::digest(&bytes);
        if expected
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(Error::Conflict.into());
        }
        if applying {
            // Two SQLite files cannot share this transaction. Keep durable lineage
            // after a coordinator failure; the identical request can finish later.
            store
                .adopt_catalog_projects_after(identity, &plan.mappings, require_new, || {
                    catalog_identity::adopt(&mut tx, identity).map_err(|error| match error {
                        catalog_identity::Error::Sqlite(error) => StoreError::Sqlite(error),
                        catalog_identity::Error::Conflict => StoreError::Conflict,
                        catalog_identity::Error::InvalidIdentity => StoreError::InvalidInput,
                        catalog_identity::Error::InvalidRecord => StoreError::CorruptDatabase,
                        catalog_identity::Error::UnsupportedVersion => {
                            StoreError::UnsupportedSchema
                        }
                    })?;
                    tx.commit().map_err(StoreError::from)
                })
                .map_err(Error::from)?;
        } else {
            tx.commit().map_err(StoreError::from).map_err(Error::from)?;
        }
        Ok::<_, AdoptionError>(Report {
            catalog_identity: identity,
            preview_digest: digest,
            mappings,
            applied: applying,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director catalog adoption worker failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(report)))
}

pub(super) enum AdoptionError {
    Api(Error),
    Discovery(catalog_discovery::DiscoveryError),
}

impl From<Error> for AdoptionError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<catalog_discovery::DiscoveryError> for AdoptionError {
    fn from(error: catalog_discovery::DiscoveryError) -> Self {
        Self::Discovery(error)
    }
}
impl From<catalog_identity::Error> for AdoptionError {
    fn from(error: catalog_identity::Error) -> Self {
        Self::Api(error.into())
    }
}
impl IntoResponse for AdoptionError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::Discovery(error) => error.into_response(),
        }
    }
}
