//! A person's flat group choices: splitting and joining one family's
//! groups, and putting lights on a side of a change or in a group.
//!
//! Every choice is a database mutation, so each write waits for the
//! database-management gate; anyone may read them in the flat groups view
//! and a light's Calibration panel. A write queues what it changes and no
//! more: a split or join asks the rig's lights again through the flat
//! measuring job (no flat is measured again) and judges calibration health
//! again; a light's choice judges health again. Stacks and masters follow
//! from the selection, so they rebuild only where a light's flats change.
//!
//! See `docs/design/flat-grouping.md` › Choices by hand.

use crate::calibration::flat_groups::{
    self, job::Rigs, job::Scope, model, override_store, overrides,
};
use crate::server::api::ApiResponse;
use crate::server::database_context::{open_scheduler_connection_with_flags, DatabaseContext};
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::state::AppState;
use axum::extract::State;
use axum::Json;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

fn open(ctx: &DatabaseContext, write: bool) -> Result<Connection, AppError> {
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
    };
    let conn =
        open_scheduler_connection_with_flags(&ctx.database_path, flags).map_err(AppError::db)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(AppError::db)?;
    Ok(conn)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| AppError::InternalError(format!("flat group choice task: {error}")))?
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Deserialize)]
pub struct GroupOverrideRequest {
    /// `split`, `join` or `reset`.
    pub choice: String,
    /// The session the edge comes before, by response key.
    #[serde(default)]
    pub after_response: Option<String>,
    /// For a join: the session on the earlier side, by response key.
    #[serde(default)]
    pub before_response: Option<String>,
    /// For a reset: the recorded edge, when no session names it any more.
    #[serde(default)]
    pub anchor_frame: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GroupOverrideResponse {
    pub anchor_frame: String,
    pub choice: Option<overrides::GroupChoice>,
}

/// A session's label: "R at 104.0°, 2026-08-12".
fn session_label(book: &model::Book, session: usize) -> String {
    let night = &book.sessions[session].row.night;
    match book
        .families
        .iter()
        .position(|family| family.sessions.contains(&session))
    {
        Some(family) => format!("{}, {night}", flat_groups::family_label(book, family)),
        None => night.clone(),
    }
}

/// Work out the row a split, join or reset records.
pub(crate) fn group_override_row(
    conn: &Connection,
    request: &GroupOverrideRequest,
) -> Result<(overrides::GroupOverrideRow, String), AppError> {
    let book = flat_groups::load_book(conn).map_err(AppError::db)?;
    let session_of = |response: &Option<String>| {
        response
            .as_deref()
            .and_then(|key| book.session_by_response(key))
    };
    let choice = match request.choice.as_str() {
        "reset" => None,
        other => Some(overrides::GroupChoice::parse(other).ok_or_else(|| {
            AppError::BadRequest(format!(
                "choice must be split, join or reset, not {other:?}"
            ))
        })?),
    };
    if choice.is_none() {
        // A reset names the recorded edge, by its flat or its session.
        let anchor = match (&request.anchor_frame, session_of(&request.after_response)) {
            (Some(anchor), _) => anchor.clone(),
            (None, Some(session)) => book.sessions[session]
                .row
                .frame_uuids
                .first()
                .cloned()
                .ok_or(AppError::NotFound)?,
            (None, None) => return Err(AppError::NotFound),
        };
        let existing = override_store::load_group_overrides(conn)
            .map_err(AppError::db)?
            .into_iter()
            .find(|row| row.anchor_frame == anchor)
            .ok_or(AppError::NotFound)?;
        let rig = existing.rig_uuid.clone();
        return Ok((
            overrides::GroupOverrideRow {
                choice: None,
                chosen_at: now(),
                ..existing
            },
            rig,
        ));
    }
    let after = session_of(&request.after_response).ok_or_else(|| {
        AppError::BadRequest("after_response names no measured flat session".into())
    })?;
    let family = book
        .families
        .iter()
        .position(|family| family.sessions.contains(&after))
        .ok_or(AppError::NotFound)?;
    let row = &book.sessions[after].row;
    let anchor_frame = row.frame_uuids.first().cloned().ok_or(AppError::NotFound)?;
    let (before_frame, before_session_key, before_night, label) = match choice {
        Some(overrides::GroupChoice::Join) => {
            let before = session_of(&request.before_response).ok_or_else(|| {
                AppError::BadRequest("a join names the session before it: before_response".into())
            })?;
            if !book.families[family].sessions.contains(&before) || before == after {
                return Err(AppError::BadRequest(
                    "a join takes two sessions of one family".into(),
                ));
            }
            let before_row = &book.sessions[before].row;
            (
                before_row.frame_uuids.first().cloned(),
                Some(before_row.session_key.clone()),
                Some(before_row.night.clone()),
                format!(
                    "{} joined with {}",
                    session_label(&book, after),
                    before_row.night
                ),
            )
        }
        _ => (
            None,
            None,
            None,
            format!("{} starts a group", session_label(&book, after)),
        ),
    };
    Ok((
        overrides::GroupOverrideRow {
            anchor_frame,
            session_key: row.session_key.clone(),
            family_key: book.families[family].key.clone(),
            rig_uuid: row.rig_uuid.clone(),
            choice,
            before_frame,
            before_session_key,
            label,
            chosen_at: now(),
            night: Some(row.night.clone()),
            before_night,
        },
        row.rig_uuid.clone(),
    ))
}

/// `PUT /api/db/{db}/calibrations/flat-groups/overrides` — split one
/// family's groups at a session, join two neighbouring groups, or reset
/// either. Behind the database-management gate.
pub async fn put_group_override(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<GroupOverrideRequest>,
) -> Result<Json<ApiResponse<GroupOverrideResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let context = Arc::clone(&ctx.0);
    let (row, rig) = blocking(move || {
        let conn = open(&context, true)?;
        let (row, rig) = group_override_row(&conn, &request)?;
        override_store::set_group_override(&conn, &row)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        Ok((row, rig))
    })
    .await?;
    // The groups and changes are worked out again on the next read; the
    // rig's lights are asked again only where a change moved, and health
    // is judged again. No flat is measured again, nothing is regraded.
    crate::server::flat_measure::queue(&state, &ctx.database_path, Scope::lights(Rigs::of([rig])));
    crate::server::calibration_health::request(&state, &ctx.database_path);
    Ok(Json(ApiResponse::success(GroupOverrideResponse {
        anchor_frame: row.anchor_frame,
        choice: row.choice,
    })))
}

#[derive(Debug, Deserialize)]
pub struct LightChoiceRequest {
    pub image_ids: Vec<i64>,
    /// Every light of the imaging sessions the named lights belong to.
    #[serde(default)]
    pub whole_session: bool,
    /// `before`, `after`, `group` or `reset`.
    pub choice: String,
    /// The change a side is chosen for, or a reset is about; a reset with
    /// none resets every choice of the lights.
    #[serde(default)]
    pub change_key: Option<String>,
    /// For a group: one of its sessions, by response key.
    #[serde(default)]
    pub group_response: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct LightChoiceResponse {
    /// Lights whose choices were set or reset.
    pub lights: usize,
    /// Lights named that have no GUID to keep a choice under.
    pub without_guid: usize,
    /// Lights named that the choice does not reach: not shot during the
    /// change, or of another rig or filter.
    pub skipped: usize,
}

/// One light a request names, as the catalog keeps it.
#[derive(Debug, Clone)]
struct NamedLight {
    guid: Option<String>,
    filter: Option<String>,
    at: Option<i64>,
    /// The rigs its profile shot with; empty when none is bound.
    rigs: Vec<String>,
}

impl NamedLight {
    fn of_rig(&self, rig: &str) -> bool {
        self.rigs.is_empty() || self.rigs.iter().any(|r| r == rig)
    }
}

/// The lights the request names: the named ones, or every light of their
/// imaging sessions.
fn named_lights(
    conn: &Connection,
    book: &model::Book,
    request: &LightChoiceRequest,
) -> Result<Vec<NamedLight>, AppError> {
    let mut ids: BTreeSet<i64> = request.image_ids.iter().copied().collect();
    if request.whole_session {
        for session in &book.imaging {
            if session
                .lights
                .iter()
                .any(|id| request.image_ids.contains(id))
            {
                ids.extend(session.lights.iter().copied());
            }
        }
    }
    let rigs_of: HashMap<i64, &Vec<String>> = book
        .imaging
        .iter()
        .flat_map(|session| session.lights.iter().map(move |id| (*id, &session.rigs)))
        .collect();
    let mut statement = conn
        .prepare("SELECT guid, filtername, acquireddate FROM acquiredimage WHERE Id = ?1")
        .map_err(|_| AppError::BadRequest("this catalog's lights keep no GUID".into()))?;
    let mut out = Vec::new();
    for id in ids {
        let row = statement
            .query_row([id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .optional()
            .map_err(AppError::db)?;
        if let Some((guid, filter, at)) = row {
            out.push(NamedLight {
                guid: guid.filter(|guid| !guid.trim().is_empty()),
                filter,
                at,
                rigs: rigs_of
                    .get(&id)
                    .map(|rigs| (*rigs).clone())
                    .unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// Whether a person may put `light` on a side of `event`: the change is of
/// its rig and reaches its filter, and the light was shot inside its
/// window, or the request names this one light alone (its Calibration
/// panel offers the changes its flats were chosen around too). A side for
/// any other light would move it against its own time.
fn side_reaches(event: &model::Event, light: &NamedLight, alone: bool) -> bool {
    light.of_rig(&event.rig_uuid)
        && event.applies_to(light.filter.as_deref())
        && (alone || light.at.is_some_and(|at| event.side_of(at).is_none()))
}

/// When a change began, on its rig's clock: "2026-08-11 06:23 UTC−7".
fn on_rig_clock(conn: &Connection, rig: &str, at: i64) -> String {
    let offset = crate::calibration::Nights::load(conn)
        .clock_offset(rig)
        .unwrap_or(0);
    let when = chrono::DateTime::from_timestamp(at + offset, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let zone = if offset == 0 {
        "UTC".to_string()
    } else {
        let minutes = offset.abs() / 60;
        let rest = minutes % 60;
        format!(
            "UTC{}{}{}",
            if offset < 0 { '−' } else { '+' },
            minutes / 60,
            if rest > 0 {
                format!(":{rest:02}")
            } else {
                String::new()
            }
        )
    };
    format!("{when} {zone}")
}

/// Work out and record the rows a light choice request makes.
pub(crate) fn apply_light_choices(
    conn: &Connection,
    request: &LightChoiceRequest,
) -> Result<LightChoiceResponse, AppError> {
    if request.image_ids.is_empty() {
        return Err(AppError::BadRequest("no lights named".into()));
    }
    let book = flat_groups::load_book(conn).map_err(AppError::db)?;
    let named = named_lights(conn, &book, request)?;
    if named.is_empty() {
        return Err(AppError::NotFound);
    }
    let alone = named.len() == 1 && request.image_ids.len() == 1;
    // The lights the choice can apply to; the rest are skipped.
    let lights: Vec<&NamedLight> = match request.choice.as_str() {
        "before" | "after" => {
            let key = request.change_key.as_deref().ok_or_else(|| {
                AppError::BadRequest("a side is chosen for a change: change_key".into())
            })?;
            let event = book
                .events
                .iter()
                .find(|event| event.key == key)
                .ok_or(AppError::NotFound)?;
            named
                .iter()
                .filter(|light| side_reaches(event, light, alone))
                .collect()
        }
        "group" => {
            let session = request
                .group_response
                .as_deref()
                .and_then(|key| book.session_by_response(key))
                .ok_or_else(|| {
                    AppError::BadRequest("group_response names no measured flat session".into())
                })?;
            let row = &book.sessions[session].row;
            named
                .iter()
                .filter(|light| {
                    light.of_rig(&row.rig_uuid)
                        && match (&light.filter, &row.filter) {
                            (Some(light), Some(flats)) => {
                                model::canonical_filter(light) == model::canonical_filter(flats)
                            }
                            _ => true,
                        }
                })
                .collect()
        }
        _ => named.iter().collect(),
    };
    if lights.is_empty() {
        return Err(AppError::BadRequest(
            "the choice reaches none of these lights: none of them was shot during that change \
             on its rig and filter, or of the group's rig and filter"
                .into(),
        ));
    }
    let guids: Vec<String> = lights
        .iter()
        .filter_map(|light| light.guid.clone())
        .collect();
    let response = LightChoiceResponse {
        lights: guids.len(),
        without_guid: lights.len() - guids.len(),
        skipped: named.len() - lights.len(),
    };
    let at = now();
    let recorded = override_store::light_choices_of(conn, &guids).map_err(AppError::db)?;
    // Reset these rows of the lights: kept with no choice, so a copy's
    // older choice does not come back.
    let reset =
        |keep: &dyn Fn(&overrides::LightChoiceRow) -> bool| -> Vec<overrides::LightChoiceRow> {
            recorded
                .iter()
                .filter(|row| row.choice.is_some() && keep(row))
                .map(|row| overrides::LightChoiceRow {
                    choice: None,
                    chosen_at: at,
                    ..row.clone()
                })
                .collect()
        };
    let rows: Vec<overrides::LightChoiceRow> = match request.choice.as_str() {
        "reset" => match &request.change_key {
            Some(key) => reset(&|row| &row.change_key == key),
            None => reset(&|_| true),
        },
        "before" | "after" => {
            let side = overrides::LightChoice::parse(&request.choice).expect("a side");
            let key = request.change_key.as_deref().unwrap_or_default();
            let event = book
                .events
                .iter()
                .find(|event| event.key == key)
                .ok_or(AppError::NotFound)?;
            let note = flat_groups::EventNote::of(event);
            let label = format!(
                "{} the change of {} ({})",
                side.as_str(),
                on_rig_clock(conn, &event.rig_uuid, event.start),
                note.what
            );
            // A side replaces a group the person chose before.
            let mut rows = reset(&|row| row.change_key == overrides::GROUP_CHOICE_KEY);
            rows.extend(guids.iter().map(|guid| overrides::LightChoiceRow {
                light_guid: guid.clone(),
                change_key: event.key.clone(),
                rig_uuid: event.rig_uuid.clone(),
                change_start: Some(event.start),
                change_end: Some(event.end),
                choice: Some(side),
                group_frame: None,
                group_session_key: None,
                label: label.clone(),
                chosen_at: at,
            }));
            rows
        }
        "group" => {
            let session = request
                .group_response
                .as_deref()
                .and_then(|key| book.session_by_response(key))
                .ok_or_else(|| {
                    AppError::BadRequest("group_response names no measured flat session".into())
                })?;
            let family = book
                .families
                .iter()
                .position(|family| family.sessions.contains(&session))
                .ok_or(AppError::NotFound)?;
            let group = book.group_of(family, session).ok_or(AppError::NotFound)?;
            let note =
                flat_groups::GroupNote::of(&book, family, group).ok_or(AppError::NotFound)?;
            let row = &book.sessions[session].row;
            // A group replaces the sides the person chose before.
            let mut rows = reset(&|row| row.change_key != overrides::GROUP_CHOICE_KEY);
            rows.extend(guids.iter().map(|guid| overrides::LightChoiceRow {
                light_guid: guid.clone(),
                change_key: overrides::GROUP_CHOICE_KEY.into(),
                rig_uuid: row.rig_uuid.clone(),
                change_start: None,
                change_end: None,
                choice: Some(overrides::LightChoice::Group),
                group_frame: row.frame_uuids.first().cloned(),
                group_session_key: Some(row.session_key.clone()),
                label: format!("the group {}", note.label()),
                chosen_at: at,
            }));
            rows
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "choice must be before, after, group or reset, not {other:?}"
            )));
        }
    };
    override_store::set_light_choices(conn, &rows)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(response)
}

/// `PUT /api/db/{db}/calibrations/flat-choices` — put lights (or every
/// light of their imaging sessions) before or after a change, give them
/// one group's flats, or reset their choices. Behind the
/// database-management gate.
pub async fn put_light_choices(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<LightChoiceRequest>,
) -> Result<Json<ApiResponse<LightChoiceResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let context = Arc::clone(&ctx.0);
    let response = blocking(move || {
        let conn = open(&context, true)?;
        apply_light_choices(&conn, &request)
    })
    .await?;
    if response.without_guid > 0 && response.lights == 0 {
        return Err(AppError::BadRequest(
            "these lights have no GUID to keep a choice under; give them GUIDs first".into(),
        ));
    }
    // Which flats the lights take changed; nothing is measured again.
    crate::server::calibration_health::request(&state, &ctx.database_path);
    Ok(Json(ApiResponse::success(response)))
}

#[derive(Debug, Deserialize)]
pub struct LightChoiceOptionsRequest {
    pub image_ids: Vec<i64>,
}

/// One change some of the lights were shot during.
#[derive(Debug, Serialize)]
pub struct BulkChangeOption {
    pub key: String,
    /// Its rig, whose clock its window reads on.
    pub rig_uuid: String,
    pub start: i64,
    pub end: i64,
    pub what: String,
    pub camera_side: bool,
    pub filters: Vec<String>,
    /// Of the named lights, those shot inside its window.
    pub lights: usize,
    /// Of those, the ones a person put before it, and after it.
    pub chosen_before: usize,
    pub chosen_after: usize,
    /// Those lights, by id: what a side for the change is sent for.
    pub image_ids: Vec<i64>,
}

#[derive(Debug, Default, Serialize)]
pub struct LightChoiceOptions {
    pub changes: Vec<BulkChangeOption>,
    /// Named lights with any choice in force.
    pub with_choices: usize,
    /// Named lights without a GUID.
    pub without_guid: usize,
}

/// The changes a set of lights were shot during, from the catalog alone:
/// each light's rigs by its profile, its filter and capture time.
pub(crate) fn light_choice_options(
    conn: &Connection,
    image_ids: &[i64],
) -> Result<LightChoiceOptions, AppError> {
    let mut options = LightChoiceOptions::default();
    let book = flat_groups::load_book(conn).map_err(AppError::db)?;
    let rigs_of: HashMap<i64, &Vec<String>> = book
        .imaging
        .iter()
        .flat_map(|session| session.lights.iter().map(move |id| (*id, &session.rigs)))
        .collect();
    let has_guid = conn
        .prepare("SELECT guid FROM acquiredimage LIMIT 0")
        .is_ok();
    let sql = if has_guid {
        "SELECT filtername, acquireddate, guid FROM acquiredimage WHERE Id = ?1"
    } else {
        "SELECT filtername, acquireddate, NULL FROM acquiredimage WHERE Id = ?1"
    };
    let mut statement = conn.prepare(sql).map_err(AppError::db)?;
    let mut changes: BTreeMap<usize, BulkChangeOption> = BTreeMap::new();
    for id in image_ids {
        let Some((filter, at, guid)) = statement
            .query_row([id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()
            .map_err(AppError::db)?
        else {
            continue;
        };
        let guid = guid.filter(|guid| !guid.trim().is_empty());
        let chosen = match &guid {
            Some(guid) => override_store::light_choices_of(conn, std::slice::from_ref(guid))
                .map_err(AppError::db)?
                .into_iter()
                .filter(|row| row.choice.is_some())
                .collect(),
            None => {
                options.without_guid += 1;
                Vec::new()
            }
        };
        if !chosen.is_empty() {
            options.with_choices += 1;
        }
        let (Some(at), Some(rigs)) = (at, rigs_of.get(id)) else {
            continue;
        };
        for (index, event) in book.events.iter().enumerate() {
            if !(rigs.is_empty() || rigs.contains(&event.rig_uuid))
                || !event.applies_to(filter.as_deref())
                || event.side_of(at).is_some()
            {
                continue;
            }
            let entry = changes.entry(index).or_insert_with(|| {
                let note = flat_groups::EventNote::of(event);
                BulkChangeOption {
                    key: event.key.clone(),
                    rig_uuid: event.rig_uuid.clone(),
                    start: event.start,
                    end: event.end,
                    what: note.what,
                    camera_side: event.camera_side,
                    filters: note.filters.iter().map(|f| f.to_uppercase()).collect(),
                    lights: 0,
                    chosen_before: 0,
                    chosen_after: 0,
                    image_ids: Vec::new(),
                }
            });
            entry.lights += 1;
            entry.image_ids.push(*id);
            let side = chosen
                .iter()
                .find(|row| row.change_key == event.key)
                .and_then(|row| row.choice);
            match side {
                Some(overrides::LightChoice::Before) => entry.chosen_before += 1,
                Some(overrides::LightChoice::After) => entry.chosen_after += 1,
                _ => {}
            }
        }
    }
    options.changes = changes.into_values().collect();
    Ok(options)
}

/// `POST /api/db/{db}/calibrations/flat-choices/options` — the changes the
/// named lights were shot during, for the Images bulk action. Reads no
/// header and no pixel.
pub async fn post_light_choice_options(
    ctx: DbContext,
    Json(request): Json<LightChoiceOptionsRequest>,
) -> Result<Json<ApiResponse<LightChoiceOptions>>, AppError> {
    let context = Arc::clone(&ctx.0);
    let options = blocking(move || {
        let conn = open(&context, false)?;
        light_choice_options(&conn, &request.image_ids)
    })
    .await?;
    Ok(Json(ApiResponse::success(options)))
}

#[cfg(test)]
mod tests;
