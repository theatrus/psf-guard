//! Framing geometry and drafts for the planning view. The shared core does
//! the arithmetic; the browser draws it. A draft is editable intent, not an
//! allocation, and the preview is stateless.

use super::*;
use psf_guard_director_core::{
    bandpass::{default_exposure_seconds, BandpassKind, ExposureContext},
    framing::{FramingPreview, FramingRequest, LayoutIssue, LayoutIssueKind},
    optics::FieldOfView,
};

/// A layout issue in a few words, for warnings and refusals.
pub(super) fn describe_issue(issue: &LayoutIssue) -> String {
    let percent = issue
        .percent
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_default();
    let pair = issue.panels.join(" and ");
    match issue.kind {
        LayoutIssueKind::ThinOverlap => format!("{pair} overlap by only {percent}"),
        LayoutIssueKind::Gap => format!("{pair} leave a gap of {percent} of a panel"),
        LayoutIssueKind::Isolated => format!("{pair} overlaps no other panel by the minimum"),
        LayoutIssueKind::Disconnected => format!(
            "the panels fall into {} groups that do not overlap, starting at {}",
            issue.panels.len(),
            issue.panels.join(", ")
        ),
    }
}
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::framing::{MeasuredGeometry, PanelPlacement};
use psf_guard_director_meta::{framing::FramingDraft, profile::RigProfile, site_profile::RigSite};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FramingRequest>,
) -> Result<Json<ApiResponse<FramingPreview>>, Error> {
    enabled(&state)?;
    let preview = request.preview().map_err(|_| Error::Invalid)?;
    Ok(Json(ApiResponse::success(preview)))
}

#[derive(Serialize)]
pub(super) struct DraftView {
    project: NamedIdentity,
    draft: Option<FramingDraft>,
}

pub(super) async fn get_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<DraftView>>, Error> {
    let view = enabled(&state)?
        .query(move |store| {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let draft = store.framing_draft(id)?;
            Ok(DraftView { project, draft })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

/// The body is the whole draft; its `revision` is the one the caller read.
pub(super) async fn put_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(mut draft): Json<FramingDraft>,
) -> Result<Json<ApiResponse<DraftView>>, Error> {
    if draft.project_id != id {
        return Err(Error::Invalid);
    }
    draft.updated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let instance_id = service.instance_id;
    // Only the server reads frames, so the shot geometry a locked panel is
    // drawn from is the server's own reading, written here whatever the
    // body carries: a client never has to echo it, and a reading that moved
    // on since the editor last asked cannot make a save fail. A panel no
    // frame speaks for now keeps the reading it had; an unlocked panel is
    // planned and has none.
    let draft = if draft.layout.has_locked()
        || draft.rig_framings.iter().any(|own| own.layout.has_locked())
    {
        service
            .clone()
            .with_reader(move |store| {
                let stored = store.framing_draft(id)?;
                let mut keying = draft.clone();
                for_each_panel(&mut keying, |rig, panel| {
                    panel.measured = stored
                        .as_ref()
                        .and_then(|stored| measured_in(stored, rig, &panel.id));
                });
                let mosaic =
                    super::mosaic::collect_for(store, id, &catalogs, instance_id, Some(&keying))?;
                let mut draft = draft;
                for_each_panel(&mut draft, |rig, panel| {
                    panel.measured = if panel.locked {
                        mosaic
                            .measured()
                            .iter()
                            .find(|m| m.framing_rig_id == rig && m.panel_id == panel.id)
                            .map(|m| m.geometry.clone())
                            .or_else(|| {
                                stored
                                    .as_ref()
                                    .and_then(|stored| measured_in(stored, rig, &panel.id))
                            })
                    } else {
                        None
                    };
                });
                Ok::<_, Error>(draft)
            })
            .await?
    } else {
        let mut draft = draft;
        for_each_panel(&mut draft, |_, panel| panel.measured = None);
        draft
    };
    let view = service
        .with_writer(move |store| -> Result<DraftView, Error> {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let expected = draft.revision;
            // Said here with the panels' names; the store refuses it too.
            if let Some(stored) = store.framing_draft(id)?
                && stored.revision == expected
            {
                let moved = stored.locked_moves(&draft);
                if !moved.is_empty() {
                    return Err(Error::Refused(locked_refusal(store, &moved)?));
                }
            }
            let saved = store.save_framing_draft(&draft, expected)?;
            Ok(DraftView {
                project,
                draft: Some(saved),
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

/// Every panel of a draft's framings, with the rig framed on its own it
/// belongs to (`None` for the shared framing).
fn for_each_panel(
    draft: &mut FramingDraft,
    mut visit: impl FnMut(Option<Uuid>, &mut PanelPlacement),
) {
    for panel in &mut draft.layout.panels {
        visit(None, panel);
    }
    for own in &mut draft.rig_framings {
        let rig = own.rig_id;
        for panel in &mut own.layout.panels {
            visit(Some(rig), panel);
        }
    }
}

/// A panel's stored shot geometry, in the shared framing or a rig's own.
fn measured_in(draft: &FramingDraft, rig: Option<Uuid>, id: &str) -> Option<MeasuredGeometry> {
    let layout = match rig {
        Some(rig) => &draft.rig_framing(rig)?.layout,
        None => &draft.layout,
    };
    layout.panel(id)?.measured.clone()
}

/// Why a save that would move locked panels is refused, in words: each
/// panel by id, a rig's own by the rig's name, and what holds them.
fn locked_refusal(store: &MetaStore, moved: &[String]) -> Result<String, Error> {
    let mut shared = Vec::new();
    let mut own: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in moved {
        match entry
            .split_once(':')
            .and_then(|(rig, id)| Uuid::parse_str(rig).ok().map(|rig| (rig, id)))
        {
            Some((rig, id)) => {
                let name = store
                    .rig(rig)?
                    .map_or_else(|| "a rig".to_owned(), |rig| rig.name);
                own.entry(name).or_default().push(id.to_owned());
            }
            None => shared.push(entry.clone()),
        }
    }
    let list = |ids: &[String]| {
        format!(
            "panel{} {}",
            if ids.len() == 1 { "" } else { "s" },
            ids.join(", ")
        )
    };
    let mut parts = Vec::new();
    if !shared.is_empty() {
        parts.push(list(&shared));
    }
    for (name, ids) in &own {
        parts.push(format!("{} of {name}'s own framing", list(ids)));
    }
    Ok(format!(
        "Locked {} would move. A locked panel keeps its place, its turn, and the center, camera angle and panel size it was placed with, including a shared center or angle a rig's own framing follows. Unlock {} first.",
        parts.join(" and "),
        if moved.len() == 1 { "it" } else { "them" },
    ))
}

#[derive(Serialize)]
pub(super) struct RigProfileSummary {
    rig: NamedIdentity,
    catalog_slug: String,
    catalog_name: String,
    profile: Option<RigProfile>,
    field_of_view: Option<FieldOfView>,
    /// Starting exposure lengths for this rig's optics and sky, per band kind.
    default_exposure_seconds: DefaultExposures,
    /// The location and horizon planning uses: the rig's own, else its site's.
    site: RigSite,
}

#[derive(Serialize)]
pub(super) struct DefaultExposures {
    broadband: f64,
    narrowband: f64,
}

fn default_exposures(
    profile: Option<&RigProfile>,
    field_of_view: Option<&FieldOfView>,
) -> DefaultExposures {
    let context = |kind| ExposureContext {
        focal_ratio: field_of_view.and_then(|fov| fov.focal_ratio),
        bortle_class: profile
            .and_then(|p| p.sky_quality.as_ref())
            .map(|sky| sky.value.bortle_class),
        kind,
    };
    DefaultExposures {
        broadband: default_exposure_seconds(context(BandpassKind::Broadband)),
        narrowband: default_exposure_seconds(context(BandpassKind::Narrowband)),
    }
}

/// Every registered database that is bound to a rig, with the rig's profile.
/// Databases that cannot be opened or are not bound are left out, not failed.
pub(super) async fn rig_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigProfileSummary>>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let summaries = service
        .clone()
        .with_reader(move |store| {
            let mut summaries = Vec::new();
            for (identity, catalog) in identified_catalogs(&catalogs, service.instance_id).iter() {
                let Some(binding) = store.catalog_rig(identity.id)? else {
                    continue;
                };
                let profile = store.rig_profile(binding.rig.id)?;
                let site = store.rig_site(binding.rig.id, profile.as_ref())?;
                let field_of_view = profile
                    .as_ref()
                    .and_then(|p| p.optics.as_ref())
                    .and_then(|optics| optics.value.field_of_view().ok());
                summaries.push(RigProfileSummary {
                    rig: binding.rig,
                    catalog_slug: catalog.id.clone(),
                    catalog_name: catalog.name.clone(),
                    default_exposure_seconds: default_exposures(
                        profile.as_ref(),
                        field_of_view.as_ref(),
                    ),
                    profile,
                    field_of_view,
                    site,
                });
            }
            summaries.sort_by(|a, b| a.catalog_name.cmp(&b.catalog_name));
            Ok::<_, Error>(summaries)
        })
        .await?;
    Ok(Json(ApiResponse::success(summaries)))
}
