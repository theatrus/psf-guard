//! Delete a rig's records when its database leaves PSF Guard. The preview
//! shows what goes. The database's removal route asks [`refusal`] first, so
//! open work stops the removal with the database still registered, then
//! changes the registry, then deletes the rig with [`remove`].

use super::*;
use psf_guard_director_meta::rig_removal::{RemovalBlocker, RigRemoval};

#[derive(Serialize)]
pub(super) struct Review {
    /// `None` when the database has no rig.
    removal: Option<RigRemoval>,
}

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<ApiResponse<Review>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    let instance = service.instance_id;
    let identity = identity_of(instance, &catalog.database_path);
    if shared(&state, &catalog, instance, &identity) {
        return Ok(Json(ApiResponse::success(Review { removal: None })));
    }
    let removal = service
        .clone()
        .with_reader(move |store| Ok::<_, Error>(store.rig_removal(identity.id)?))
        .await?;
    Ok(Json(ApiResponse::success(Review { removal })))
}

/// Whether another registered database carries `catalog`'s identity, and so
/// its rig: the same file under two names, or a copy of it. Removing one of
/// them leaves the rig to the other.
fn shared(
    state: &AppState,
    catalog: &DatabaseContext,
    instance: Uuid,
    identity: &CatalogIdentity,
) -> bool {
    let others: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .filter(|other| other.id != catalog.id)
        .cloned()
        .collect();
    others
        .iter()
        .any(|other| identity_of(instance, &other.database_path).id == identity.id)
}

const OPEN_WORK: &str =
    "This rig has open work. Open Rig setup, choose Close work on the Director tab, then remove it.";

/// Why the database's rig stops its removal, if anything does. A Director
/// store that cannot be read does not: the database can still be removed,
/// and [`remove`] then deletes what it can.
pub(crate) async fn refusal(state: &Arc<AppState>, catalog: &DatabaseContext) -> Option<String> {
    let service = enabled(state).ok()?;
    let instance = service.instance_id;
    let identity = identity_of(instance, &catalog.database_path);
    if shared(state, catalog, instance, &identity) {
        return None;
    }
    match service
        .clone()
        .with_reader(move |store| Ok::<_, Error>(store.rig_removal(identity.id)?))
        .await
    {
        Ok(Some(review)) => review
            .blocker
            .map(|RemovalBlocker::OpenWork| OPEN_WORK.into()),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(?error, "A rig's records could not be read before removal");
            None
        }
    }
}

/// Why a database's rig could not be removed.
pub(crate) enum RemovalError {
    /// A reason to show the person, who can act on it and try again.
    Refused(String),
    Failed(String),
}

/// Delete the rig of a database that is leaving PSF Guard. Answers whether a
/// rig was deleted: a database with no rig, or a server without Director,
/// has nothing to delete. A rig with collaboration records keeps them, since
/// they can hold reports not sent yet, and adding the file back restores it.
pub(crate) async fn remove(
    state: &Arc<AppState>,
    catalog: Arc<DatabaseContext>,
) -> Result<bool, RemovalError> {
    let Ok(service) = enabled(state) else {
        return Ok(false);
    };
    let instance = service.instance_id;
    let identity = identity_of(instance, &catalog.database_path);
    if shared(state, &catalog, instance, &identity) {
        return Ok(false);
    }
    let result = service
        .clone()
        .with_writer(move |store| {
            let Some(review) = store.rig_removal(identity.id)? else {
                return Ok::<_, Error>(Ok(false));
            };
            if let Some(blocker) = review.blocker {
                return Ok(Err(blocker));
            }
            if review.collaboration {
                return Ok(Ok(false));
            }
            store.remove_rig(identity.id)?;
            Ok(Ok(true))
        })
        .await;
    match result {
        Ok(Ok(removed)) => Ok(removed),
        Ok(Err(RemovalBlocker::OpenWork)) => Err(RemovalError::Refused(OPEN_WORK.into())),
        Err(Error::Conflict) => Err(RemovalError::Refused(
            "This rig changed while it was being removed. Try again.".into(),
        )),
        Err(error) => {
            tracing::warn!(?error, "A rig's records could not be removed");
            Err(RemovalError::Failed(
                "The rig's records could not be removed.".into(),
            ))
        }
    }
}
