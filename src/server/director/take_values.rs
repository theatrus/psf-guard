//! Take one rig's Target Scheduler values into the plan: the reverse of an
//! activation, for when N.I.N.A.'s rows changed after the plan reached them
//! and they are the ones to keep. The rig's linked project is read the way
//! an import reads it, and its exposure plans become that rig's part of the
//! plan: the frames it asks for, the template and length, which bands are on.
//! The framing follows the project's targets only where no other rig shares
//! it. What the plan has no place for (a Draft project, a target turned off,
//! scheduling limits) is named and left as it is.

use super::activation::ActivationError;
use super::import_drafts::{self, Drafts, Missing};
use super::*;
use psf_guard_director_core::{
    framing::{Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    framing::FramingDraft,
    plan::{Contribution, Goal, Objective, PlanDraft},
};
use rusqlite::{OpenFlags, OptionalExtension};
use std::{collections::BTreeSet, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    rig_id: Uuid,
    /// The draft revisions the person last saw; a save since refuses.
    plan_revision: u64,
    framing_revision: u64,
}

#[derive(Serialize)]
pub(super) struct Response {
    plan_revision: u64,
    framing_revision: u64,
    /// What came in from Target Scheduler, one line each.
    taken: Vec<String>,
    /// What stayed as planned, and why.
    left: Vec<String>,
}

/// Positions within this many degrees (1″) are the same place; a framing
/// Director laid out and read back again lands this close.
const SAME_PLACE_DEGREES: f64 = 1.0 / 3600.0;
const SAME_ANGLE_DEGREES: f64 = 0.01;

pub(super) async fn take(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<Request>,
) -> Result<Json<ApiResponse<Response>>, ActivationError> {
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let instance = service.instance_id;
    let rig = request.rig_id;
    let found = service
        .clone()
        .with_reader(move |store| -> Result<_, ActivationError> {
            store.project(id)?.ok_or(Error::Missing)?;
            let plan = store
                .plan_draft(id)?
                .ok_or(ActivationError::NotReady("Save a plan first."))?;
            let framing = store.framing_draft(id)?;
            let mut bound = None;
            for (identity, context) in identified_catalogs(&catalogs, instance).iter() {
                if store
                    .catalog_rig(identity.id)?
                    .is_some_and(|binding| binding.rig.id == rig)
                {
                    bound = Some((identity.id, context.clone()));
                    break;
                }
            }
            let (catalog, context) = bound.ok_or(ActivationError::NotReady(
                "This rig has no registered database on this server.",
            ))?;
            let mut after = None;
            let source = loop {
                let page = store.catalog_project_mappings(catalog, after, 256)?;
                if let Some(mapping) = page.items.iter().find(|m| m.project_id == id) {
                    break Some(mapping.source_project_guid);
                }
                match page.next_after {
                    Some(next) => after = Some(next),
                    None => break None,
                }
            };
            let source = source.ok_or(ActivationError::NotReady(
                "This rig has no Target Scheduler project linked to this plan yet.",
            ))?;
            let panel = store
                .rig_profile(rig)?
                .and_then(|profile| profile.optics)
                .and_then(|optics| optics.value.field_of_view().ok())
                .map(|fov| PanelSize {
                    width_degrees: fov.width_degrees,
                    height_degrees: fov.height_degrees,
                });
            Ok((plan, framing, context.database_path.clone(), source, panel))
        })
        .await?;
    let (plan, framing, path, source, field) = found;
    // The layout the plan has was drawn at its saved panel size; reading the
    // targets back at the same size finds the same grid.
    let panel = framing
        .as_ref()
        .and_then(|f| f.rig_framing(rig).and_then(|own| own.panel).or(f.panel))
        .or(field);
    if plan.revision != request.plan_revision
        || framing.as_ref().map_or(0, |f| f.revision) != request.framing_revision
    {
        return Err(Error::Conflict.into());
    }
    // The rig database is read on its own, with nothing of the store held.
    let read = tokio::task::spawn_blocking(move || read_rig(&path, rig, id, source, panel))
        .await
        .map_err(|_| Error::Internal)??;
    let merged = merge(&plan, framing.as_ref(), rig, read);
    let (plan_expected, framing_expected) =
        (plan.revision, framing.as_ref().map_or(0, |f| f.revision));
    let response = service
        .with_writer(move |store| -> Result<Response, ActivationError> {
            // Both revisions are checked before either draft is saved, in
            // the writer's one turn, so a take never lands half.
            let plan_now = store.plan_draft(id)?.map_or(0, |p| p.revision);
            let framing_now = store.framing_draft(id)?.map_or(0, |f| f.revision);
            if plan_now != plan_expected || framing_now != framing_expected {
                return Err(Error::Conflict.into());
            }
            let mut plan_revision = plan_now;
            let mut framing_revision = framing_now;
            if let Some(plan) = merged.plan {
                plan_revision = store.save_plan_draft(&plan, plan_now)?.revision;
            }
            if let Some(framing) = merged.framing {
                framing_revision = store.save_framing_draft(&framing, framing_now)?.revision;
            }
            Ok(Response {
                plan_revision,
                framing_revision,
                taken: merged.taken,
                left: merged.left,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(response)))
}

/// The rig's project as an import reads it, whether the read left any
/// exposure plan out (a template Director cannot plan with), and what the
/// plan has no place for.
struct Read {
    drafts: Drafts,
    left_out: bool,
    draft_project: bool,
    targets_off: i64,
}

fn read_rig(
    path: &str,
    rig: Uuid,
    project: Uuid,
    source: Uuid,
    panel: Option<PanelSize>,
) -> Result<Read, ActivationError> {
    let unreadable =
        |_| ActivationError::NotReady("This rig's database could not be read just now; try again.");
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        FilePath::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unreadable)?;
    connection
        .busy_timeout(Duration::from_secs(1))
        .map_err(unreadable)?;
    let row: Option<(i64, Option<String>, i64)> = connection
        .query_row(
            "SELECT Id, name, IFNULL(state, 0) FROM project WHERE guid=?1",
            [source.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(unreadable)?;
    let (row, name, state) = row.ok_or(ActivationError::NotReady(
        "The linked Target Scheduler project is gone from this rig's database.",
    ))?;
    let targets_off: i64 = connection
        .query_row(
            "SELECT count(*) FROM target WHERE projectid=?1 AND active=0",
            [row],
            |row| row.get(0),
        )
        .map_err(unreadable)?;
    let templates = super::plan::read_templates(&connection).map_err(|_| {
        ActivationError::NotReady("This rig's exposure templates could not be read.")
    })?;
    let drafts = import_drafts::read_drafts(
        &connection,
        Some(&templates),
        rig,
        project,
        row,
        name.as_deref().unwrap_or("Project"),
        panel,
        Missing {
            framing: true,
            plan: true,
        },
    )
    .map_err(|_| {
        ActivationError::NotReady("This rig's Target Scheduler project could not be read.")
    })?;
    let left_out = !drafts.warnings.is_empty();
    Ok(Read {
        drafts,
        left_out,
        draft_project: state == 0,
        targets_off,
    })
}

/// The drafts after a take: `None` where nothing changed.
struct Merged {
    plan: Option<PlanDraft>,
    framing: Option<FramingDraft>,
    taken: Vec<String>,
    left: Vec<String>,
}

fn goal_text(goal: Goal) -> String {
    match goal {
        Goal::Frames { value } => format!("{value} frames"),
        Goal::Hours { value } => format!("{value} h"),
    }
}

fn frames_of(goal: Goal, exposure_seconds: f64) -> Option<u32> {
    match goal {
        Goal::Frames { value } => Some(value),
        Goal::Hours { value } => {
            psf_guard_director_core::bandpass::frames_for_hours(value, exposure_seconds)
        }
    }
}

fn same_place(a: &IcrsPosition, b: &IcrsPosition) -> bool {
    let ra = ((a.ra_degrees - b.ra_degrees + 540.0).rem_euclid(360.0) - 180.0).abs()
        * b.dec_degrees.to_radians().cos();
    ra <= SAME_PLACE_DEGREES && (a.dec_degrees - b.dec_degrees).abs() <= SAME_PLACE_DEGREES
}

fn same_angle(a: f64, b: f64) -> bool {
    let apart = (a - b).rem_euclid(360.0);
    apart.min(360.0 - apart) <= SAME_ANGLE_DEGREES
}

/// Fold the rig's Target Scheduler values into the plan. Where only this
/// rig shoots a band, its frames become the band's goal; where other rigs
/// share it, they become this rig's own goal. A band the project no longer
/// shoots is turned off for this rig, unless the read left some exposure
/// plan out, when nothing is turned off. The framing follows the project's
/// targets when they moved and no other rig uses the same layout.
fn merge(plan: &PlanDraft, framing: Option<&FramingDraft>, rig: Uuid, read: Read) -> Merged {
    let mut taken = Vec::new();
    let mut left = Vec::new();
    let mut next = plan.clone();
    let source = read
        .drafts
        .plan
        .unwrap_or_else(|| PlanDraft::empty(plan.project_id, 0));
    let mut shot = BTreeSet::new();
    for objective in &source.objectives {
        let Some(read_part) = source
            .contributions
            .iter()
            .find(|c| c.objective_id == objective.id)
        else {
            continue;
        };
        shot.insert(objective.bandpass_id.clone());
        let label = &read_part.template.filter_name;
        let Some(index) = next
            .objectives
            .iter()
            .position(|o| o.bandpass_id == objective.bandpass_id)
        else {
            let id = Uuid::new_v4();
            next.objectives.push(Objective {
                id,
                ..objective.clone()
            });
            next.contributions.push(Contribution {
                id: Uuid::new_v4(),
                objective_id: id,
                ..read_part.clone()
            });
            taken.push(format!(
                "{label}: added, {} with {}",
                goal_text(objective.goal),
                read_part.template.name
            ));
            continue;
        };
        let planned = next.objectives[index].clone();
        let shared = next
            .contributions
            .iter()
            .any(|c| c.objective_id == planned.id && c.rig_id != rig && c.enabled);
        let mut notes = Vec::new();
        // Goals compare by the frames they come to at the length the rig
        // shoots, so six hours of 300 s frames and 72 frames are the same.
        let wanted = frames_of(objective.goal, read_part.exposure_seconds);
        let band_frames = frames_of(planned.goal, read_part.exposure_seconds);
        let part = next
            .contributions
            .iter()
            .position(|c| c.objective_id == planned.id && c.rig_id == rig);
        let has = part.map(|i| {
            frames_of(
                next.contributions[i].goal_for(&planned),
                read_part.exposure_seconds,
            )
        });
        let own_goal = if has.unwrap_or(band_frames) == wanted {
            part.and_then(|i| next.contributions[i].goal)
        } else if shared {
            notes.push(format!("{} on this rig", goal_text(objective.goal)));
            (band_frames != wanted).then_some(objective.goal)
        } else {
            notes.push(goal_text(objective.goal));
            next.objectives[index].goal = objective.goal;
            None
        };
        match part.map(|i| &mut next.contributions[i]) {
            Some(part) => {
                part.goal = own_goal;
                let same_template = match (
                    part.template.template_guid,
                    read_part.template.template_guid,
                ) {
                    (Some(a), Some(b)) => a == b,
                    _ => part.template.template_id == read_part.template.template_id,
                };
                if !same_template {
                    notes.push(format!("template {}", read_part.template.name));
                    part.template = read_part.template.clone();
                }
                if (part.exposure_seconds - read_part.exposure_seconds).abs() > 1e-6 {
                    notes.push(format!("{} s", read_part.exposure_seconds));
                    part.exposure_seconds = read_part.exposure_seconds;
                }
                if !part.enabled {
                    notes.push("on".into());
                    part.enabled = true;
                }
            }
            None => {
                next.contributions.push(Contribution {
                    id: Uuid::new_v4(),
                    objective_id: planned.id,
                    goal: own_goal,
                    ..read_part.clone()
                });
                notes.push(format!("shot here with {}", read_part.template.name));
            }
        }
        if !notes.is_empty() {
            taken.push(format!("{label}: {}", notes.join(", ")));
        }
    }
    // Bands this rig shoots in the plan that the project no longer has.
    let bands: Vec<(Uuid, String)> = next
        .objectives
        .iter()
        .map(|o| (o.id, o.bandpass_id.clone()))
        .collect();
    for part in next
        .contributions
        .iter_mut()
        .filter(|c| c.rig_id == rig && c.enabled)
    {
        let Some((_, band)) = bands.iter().find(|(id, _)| *id == part.objective_id) else {
            continue;
        };
        if shot.contains(band) {
            continue;
        }
        if read.left_out {
            left.push(format!(
                "{}: kept on; Target Scheduler has exposure plans Director cannot read",
                part.template.filter_name
            ));
        } else {
            taken.push(format!(
                "{}: off, as in Target Scheduler",
                part.template.filter_name
            ));
            part.enabled = false;
        }
    }
    if read.draft_project {
        left.push("Project: a Draft in Target Scheduler; an activation makes it Active".into());
    }
    if read.targets_off > 0 {
        left.push(format!(
            "{} target{} off in Target Scheduler; an activation turns {} on",
            read.targets_off,
            if read.targets_off == 1 { "" } else { "s" },
            if read.targets_off == 1 { "it" } else { "them" }
        ));
    }

    // The framing, where the project's targets moved and only this rig
    // uses the layout.
    let mut framing_next = None;
    if let (Some(read_framing), Some(stored)) = (read.drafts.framing.as_ref(), framing) {
        let mut layout = stored.clone();
        let moved = |center: Option<&IcrsPosition>, angle: Option<f64>, mosaic: &Mosaic| {
            !center.is_some_and(|c| same_place(c, &read_framing.center))
                || !angle.is_some_and(|a| same_angle(a, read_framing.position_angle_degrees))
                || *mosaic != read_framing.mosaic
        };
        if let Some(own) = layout.rig_framings.iter_mut().find(|f| f.rig_id == rig) {
            if moved(own.center.as_ref(), own.position_angle_degrees, &own.mosaic) {
                own.center = Some(read_framing.center);
                own.position_angle_degrees = Some(read_framing.position_angle_degrees);
                own.mosaic = read_framing.mosaic;
                taken.push("Framing: this rig's own, where Target Scheduler has it".into());
                framing_next = Some(layout);
            }
        } else if moved(
            Some(&layout.center),
            Some(layout.position_angle_degrees),
            &layout.mosaic,
        ) {
            let shared = next
                .contributions
                .iter()
                .any(|c| c.enabled && c.rig_id != rig && stored.rig_framing(c.rig_id).is_none());
            if shared {
                left.push("Framing: other rigs share it, so it stays as planned".into());
            } else {
                layout.center = read_framing.center;
                layout.position_angle_degrees = read_framing.position_angle_degrees;
                layout.mosaic = read_framing.mosaic;
                taken.push("Framing: where Target Scheduler has it".into());
                framing_next = Some(layout);
            }
        }
    }
    let changed = next != *plan;
    Merged {
        plan: changed.then_some(next),
        framing: framing_next,
        taken,
        left,
    }
}
