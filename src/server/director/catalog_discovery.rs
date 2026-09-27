//! Read-only TS exchange discovery. Local row IDs locate evidence, never identity.

use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::Path,
    time::Duration,
};

const MAX_PROJECTS: usize = 4096;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Issue {
    MissingProjectGuid,
    InvalidProjectGuid,
    DuplicateProjectGuid,
    MissingProfileId,
    InvalidProfileId,
    InvalidProjectName,
}

#[derive(Debug, Serialize)]
struct Project {
    source_row_id: i64,
    source_project_guid: Option<Uuid>,
    source_profile_id: Option<String>,
    name: Option<String>,
    issues: Vec<Issue>,
}

#[derive(Debug, Serialize)]
struct Profile {
    source_profile_id: String,
    project_count: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct Evidence {
    has_project_guid: bool,
    has_profile_id: bool,
    projects: Vec<Project>,
    profiles: Vec<Profile>,
}

impl Evidence {
    pub(super) fn mapping_names(
        &self,
        mappings: &[psf_guard_director_meta::catalog::ProjectMapping],
    ) -> Result<Vec<String>, Error> {
        mappings
            .iter()
            .map(|mapping| {
                let project = self
                    .projects
                    .iter()
                    .find(|project| {
                        project.source_project_guid == Some(mapping.source_project_guid)
                    })
                    .ok_or(Error::Conflict)?;
                if !project.issues.is_empty()
                    || project.source_profile_id.as_deref()
                        != Some(mapping.source_profile_id.as_str())
                {
                    return Err(Error::Conflict);
                }
                project.name.clone().ok_or(Error::Conflict)
            })
            .collect()
    }
}

#[derive(Serialize)]
pub(super) struct Discovery {
    catalog_slug: String,
    catalog_name: String,
    catalog_identity: Option<psf_guard_director_meta::CatalogIdentity>,
    // An optimistic-preview token, not a catalog identity or execution authority.
    snapshot_digest: String,
    evidence: Evidence,
}

#[derive(Debug)]
pub(super) enum DiscoveryError {
    Api(Error),
    UnsupportedSchema,
    TooManyProjects,
    MissingCatalog,
}

impl From<Error> for DiscoveryError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<rusqlite::Error> for DiscoveryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Api(StoreError::Sqlite(error).into())
    }
}
impl IntoResponse for DiscoveryError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Api(error) => return error.into_response(),
            Self::UnsupportedSchema => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Catalog has no supported project table",
            ),
            Self::TooManyProjects => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Catalog discovery supports at most 4096 projects; no partial result was returned",
            ),
            Self::MissingCatalog => (StatusCode::NOT_FOUND, "Catalog not found"),
        };
        (status, Json(ApiResponse::<()>::error(message.into()))).into_response()
    }
}

pub(super) async fn discover(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Result<Json<ApiResponse<Discovery>>, DiscoveryError> {
    let service = enabled(&state)?;
    let catalog = state
        .get_database(&slug)
        .ok_or(DiscoveryError::MissingCatalog)?;
    let permit = service
        .discovery_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let result = tokio::task::spawn_blocking(move || {
        // Cancellation keeps admission held until the SQLite read actually ends.
        let _permit = permit;
        let (evidence, catalog_identity) = read_snapshot(Path::new(&catalog.database_path))?;
        let bytes =
            serde_json::to_vec(&(&evidence, catalog_identity)).map_err(|_| Error::Internal)?;
        let snapshot_digest = digest(&bytes);
        Ok::<_, DiscoveryError>(Discovery {
            catalog_slug: catalog.id.clone(),
            catalog_name: catalog.name.clone(),
            catalog_identity,
            snapshot_digest,
            evidence,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director catalog discovery worker failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(result)))
}

pub(super) fn digest(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}

#[cfg(test)]
fn read_catalog(path: &Path) -> Result<Evidence, DiscoveryError> {
    read_snapshot(path).map(|(evidence, _)| evidence)
}

fn read_snapshot(
    path: &Path,
) -> Result<(Evidence, Option<psf_guard_director_meta::CatalogIdentity>), DiscoveryError> {
    let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // Interactive discovery should return retryable busy instead of waiting for
    // the full sync timeout. No shared request connection or meta lock is held.
    connection.busy_timeout(Duration::from_secs(2))?;
    let tx = connection.transaction()?;
    let evidence = read_evidence(&tx)?;
    let identity = crate::catalog_identity::read(&tx).map_err(Error::from)?;
    tx.commit()?;
    Ok((evidence, identity))
}

pub(super) fn read_evidence(connection: &Connection) -> Result<Evidence, DiscoveryError> {
    if !connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project' COLLATE NOCASE)",
        [], |row| row.get::<_, bool>(0),
    )? {
        return Err(DiscoveryError::UnsupportedSchema);
    }
    let mut columns = connection.prepare("PRAGMA table_info(project)")?;
    let columns = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let has = |name: &str| {
        columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(name))
    };
    if !has("id") || !has("name") {
        return Err(DiscoveryError::UnsupportedSchema);
    }
    let has_project_guid = has("guid");
    let has_profile_id = has("profileId");
    let guid = if has_project_guid { "guid" } else { "NULL" };
    let profile = if has_profile_id { "profileId" } else { "NULL" };
    let mut statement = connection.prepare(&format!(
        "SELECT Id, name, {guid}, {profile} FROM project ORDER BY Id LIMIT ?1"
    ))?;
    let mut rows = statement.query([(MAX_PROJECTS + 1) as i64])?;
    let mut projects = Vec::new();
    let mut guid_counts = HashMap::new();
    let mut profile_counts = BTreeMap::new();
    while let Some(row) = rows.next()? {
        if projects.len() == MAX_PROJECTS {
            return Err(DiscoveryError::TooManyProjects);
        }
        let mut issues = Vec::new();
        let name = text(row.get_ref(1)?);
        if name.is_none() {
            issues.push(Issue::InvalidProjectName);
        }
        let raw_guid = row.get_ref(2)?;
        let source_project_guid = text(raw_guid)
            .and_then(|s| Uuid::parse_str(&s).ok())
            .filter(|id| !id.is_nil());
        if let Some(guid) = source_project_guid {
            *guid_counts.entry(guid).or_insert(0usize) += 1;
        } else {
            issues.push(if matches!(raw_guid, ValueRef::Null) {
                Issue::MissingProjectGuid
            } else {
                Issue::InvalidProjectGuid
            });
        }
        let raw_profile = row.get_ref(3)?;
        let source_profile_id = text(raw_profile);
        if let Some(profile) = &source_profile_id {
            *profile_counts.entry(profile.clone()).or_insert(0usize) += 1;
        } else {
            issues.push(if matches!(raw_profile, ValueRef::Null) {
                Issue::MissingProfileId
            } else {
                Issue::InvalidProfileId
            });
        }
        projects.push(Project {
            source_row_id: row.get(0).map_err(|_| DiscoveryError::UnsupportedSchema)?,
            source_project_guid,
            source_profile_id,
            name,
            issues,
        });
    }
    for project in &mut projects {
        if project
            .source_project_guid
            .is_some_and(|id| guid_counts[&id] > 1)
        {
            project.issues.push(Issue::DuplicateProjectGuid);
        }
    }
    Ok(Evidence {
        has_project_guid,
        has_profile_id,
        projects,
        profiles: profile_counts
            .into_iter()
            .map(|(source_profile_id, project_count)| Profile {
                source_profile_id,
                project_count,
            })
            .collect(),
    })
}

fn text(value: ValueRef<'_>) -> Option<String> {
    let ValueRef::Text(bytes) = value else {
        return None;
    };
    if bytes.len() > MAX_TEXT_BYTES {
        return None;
    }
    let value = std::str::from_utf8(bytes).ok()?;
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests;
