//! Bind one registered project database to its rig through explicit review.
use super::catalog_adoption::AdoptionError;
use super::*;
use crate::catalog_identity;
use psf_guard_director_meta::{catalog_rig::CatalogRig, CatalogIdentity};
use rusqlite::{OpenFlags, TransactionBehavior};
use std::time::Duration;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    catalog_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Apply {
    plan: Plan,
    preview_digest: String,
}

#[derive(Serialize)]
pub(super) struct Report {
    binding: CatalogRig,
    preview_digest: String,
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
    // A preview reads the file; applying writes its identity table.
    let service = if expected.is_some() {
        writable(&state)?
    } else {
        enabled(&state)?
    };
    if plan.catalog_id.is_nil() {
        return Err(Error::Invalid.into());
    }
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let result = service
        .clone()
        .with_writer(move |store| {
            let _catalog_permit = catalog_permit;
            let applying = expected.is_some();
            let flags = if applying {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            };
            let mut connection =
                super::super::database_context::open_scheduler_connection_with_flags(
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
            let saved = catalog_identity::read(&tx)?;
            let require_new = saved.is_none();
            let identity = saved.unwrap_or(CatalogIdentity {
                id: plan.catalog_id,
                origin_instance_id: service.instance_id,
            });
            if identity.id != plan.catalog_id {
                return Err(Error::Conflict.into());
            }
            let evidence = catalog_discovery::read_evidence(&tx)?;
            let binding = store
                .preview_catalog_rig(identity, &catalog.name, require_new)
                .map_err(binding_error)?;
            let bytes = serde_json::to_vec(&(
                "catalog-rig-v1",
                &plan,
                &binding,
                &evidence,
                service.instance_id,
                &catalog.id,
                &catalog.name,
                &catalog.database_path,
            ))
            .map_err(|_| Error::Internal)?;
            let digest = catalog_discovery::digest(&bytes);
            if expected.as_ref().is_some_and(|value| value != &digest) {
                return Err(Error::Conflict.into());
            }
            if applying {
                store
                    .bind_catalog_rig_after(identity, &catalog.name, require_new, || {
                        catalog_identity::adopt(&mut tx, identity).map_err(
                            |error| match error {
                                catalog_identity::Error::Sqlite(error) => StoreError::Sqlite(error),
                                catalog_identity::Error::Conflict => StoreError::Conflict,
                                catalog_identity::Error::InvalidIdentity => {
                                    StoreError::InvalidInput
                                }
                                catalog_identity::Error::InvalidRecord => {
                                    StoreError::CorruptDatabase
                                }
                                catalog_identity::Error::UnsupportedVersion => {
                                    StoreError::UnsupportedSchema
                                }
                            },
                        )?;
                        tx.commit().map_err(StoreError::from)
                    })
                    .map_err(binding_error)?;
            } else {
                tx.commit().map_err(StoreError::from).map_err(Error::from)?;
            }
            Ok::<_, AdoptionError>(Report {
                binding,
                preview_digest: digest,
                applied: applying,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(result)))
}

fn binding_error(error: StoreError) -> AdoptionError {
    match error {
        StoreError::Conflict => AdoptionError::RigConflict,
        other => Error::from(other).into(),
    }
}
