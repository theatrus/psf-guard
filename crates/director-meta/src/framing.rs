//! The mutable framing draft of a global project: where it points, how it is
//! turned, and which panels cover it. A draft is what the operator is still
//! editing; activation later freezes intent from it. It grants nothing.
//!
//! Panels are stored one by one, each with a stable id, its place and a
//! lock (see [`Layout`]). Drafts saved before schema 29 held a rows-by-
//! columns `mosaic`; the store rewrote each into the panels that grid makes,
//! in the same places, and a body that still sends `mosaic` is read the
//! same way.

use super::*;
use psf_guard_director_core::{
    framing::{layout_or_grid, FramingRequest, Layout, Mosaic, PanelSize, TangentPlane},
    visibility::IcrsPosition,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(try_from = "FramingDraftBody")]
pub struct FramingDraft {
    pub project_id: Uuid,
    /// Compare-and-set revision; every accepted change increments it.
    pub revision: u64,
    pub target_name: String,
    pub center: IcrsPosition,
    pub position_angle_degrees: f64,
    /// The shared framing's panels, placed around `center`.
    pub layout: Layout,
    /// The rig whose field defines one panel, when one is chosen.
    #[serde(deserialize_with = "Option::deserialize")]
    pub panel_rig_id: Option<Uuid>,
    /// The panel size in use; copied from the rig when one is chosen so the
    /// draft still reads the same after that rig's profile changes.
    #[serde(deserialize_with = "Option::deserialize")]
    pub panel: Option<PanelSize>,
    /// Rigs whose fields the view overlays for comparison.
    pub shown_rig_ids: Vec<Uuid>,
    pub survey_id: String,
    /// Width of the view in degrees.
    pub view_fov_degrees: f64,
    pub updated_at_ms: u64,
    /// Rigs framed on their own over the same target: their own panels,
    /// panel size and camera angle. A rig not listed shoots the shared
    /// framing.
    #[serde(default)]
    pub rig_framings: Vec<RigFraming>,
    /// The revision that last changed what activation writes: the target,
    /// angle, panels' places and size. The view's survey, width and compared
    /// rigs, the minimum overlap and the locks move only `revision`, so
    /// choosing a sky map asks for no activation. Set by the store; a draft
    /// saved before it existed reads its revision.
    #[serde(default)]
    pub layout_revision: u64,
}

/// A draft as sent or stored: the panels as a `layout`, or, before schema
/// 29 and from a client written before then, as a `mosaic` grid.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FramingDraftBody {
    project_id: Uuid,
    revision: u64,
    target_name: String,
    center: IcrsPosition,
    position_angle_degrees: f64,
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    mosaic: Option<Mosaic>,
    #[serde(deserialize_with = "Option::deserialize")]
    panel_rig_id: Option<Uuid>,
    #[serde(deserialize_with = "Option::deserialize")]
    panel: Option<PanelSize>,
    shown_rig_ids: Vec<Uuid>,
    survey_id: String,
    view_fov_degrees: f64,
    updated_at_ms: u64,
    #[serde(default)]
    rig_framings: Vec<RigFraming>,
    #[serde(default)]
    layout_revision: u64,
}

impl TryFrom<FramingDraftBody> for FramingDraft {
    type Error = String;

    fn try_from(body: FramingDraftBody) -> Result<Self, Self::Error> {
        Ok(Self {
            project_id: body.project_id,
            revision: body.revision,
            target_name: body.target_name,
            center: body.center,
            position_angle_degrees: body.position_angle_degrees,
            layout: layout_or_grid(body.layout, body.mosaic)?,
            panel_rig_id: body.panel_rig_id,
            panel: body.panel,
            shown_rig_ids: body.shown_rig_ids,
            survey_id: body.survey_id,
            view_fov_degrees: body.view_fov_degrees,
            updated_at_ms: body.updated_at_ms,
            rig_framings: body.rig_framings,
            layout_revision: body.layout_revision,
        })
    }
}

/// How far apart two angles or coordinates may be, in degrees, and still
/// lay out the same targets: far below a pixel, and far above the float
/// drift a browser's arithmetic adds to a turn and its undoing.
const LAYOUT_TOLERANCE_DEGREES: f64 = 1e-7;
/// The same for a panel's place, in panel sizes: a ten-millionth of a
/// panel is far below a pixel for any field a camera sees.
const PLACE_TOLERANCE_PANELS: f64 = 1e-7;

/// Two angles, or two right ascensions, the same within the tolerance,
/// counting 359.99999999 and 0 as neighbours.
fn same_angle(a: f64, b: f64) -> bool {
    let apart = (a - b).rem_euclid(360.0);
    apart.min(360.0 - apart) <= LAYOUT_TOLERANCE_DEGREES
}

fn same_position(a: &IcrsPosition, b: &IcrsPosition) -> bool {
    same_angle(a.ra_degrees, b.ra_degrees)
        && (a.dec_degrees - b.dec_degrees).abs() <= LAYOUT_TOLERANCE_DEGREES
}

/// Whether two layouts put the same panels in the same places: the same
/// ids, each where it was. Order, locks and the minimum overlap do not
/// change what activation writes, nor does a shot panel's frame count or
/// how sure its reading is, only where it puts the panel.
pub fn same_places(a: &Layout, b: &Layout) -> bool {
    a.panels.len() == b.panels.len()
        && a.panels.iter().all(|panel| {
            b.panel(&panel.id).is_some_and(|other| {
                (panel.x - other.x).abs() <= PLACE_TOLERANCE_PANELS
                    && (panel.y - other.y).abs() <= PLACE_TOLERANCE_PANELS
                    && same_angle(panel.angle_offset_degrees, other.angle_offset_degrees)
                    // A panel redrawn from new solves of its frames moves.
                    && match (&panel.measured, &other.measured) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a.same_place(b),
                        _ => false,
                    }
            })
        })
}

fn same_rig_framing(a: &RigFraming, b: &RigFraming) -> bool {
    a.rig_id == b.rig_id
        && match (&a.center, &b.center) {
            (Some(a), Some(b)) => same_position(a, b),
            (None, None) => true,
            _ => false,
        }
        && match (a.position_angle_degrees, b.position_angle_degrees) {
            (Some(a), Some(b)) => same_angle(a, b),
            (None, None) => true,
            _ => false,
        }
        && same_places(&a.layout, &b.layout)
        && a.panel == b.panel
}

impl FramingDraft {
    /// Whether two drafts lay out the same targets for activation. Centers
    /// and angles compare within a small tolerance, so a quarter turn and
    /// back, which leaves a few units in the last place, asks for no
    /// activation.
    pub fn same_layout(&self, other: &Self) -> bool {
        self.target_name == other.target_name
            && same_position(&self.center, &other.center)
            && same_angle(self.position_angle_degrees, other.position_angle_degrees)
            && same_places(&self.layout, &other.layout)
            && self.panel_rig_id == other.panel_rig_id
            && self.panel == other.panel
            && self.rig_framings.len() == other.rig_framings.len()
            && self
                .rig_framings
                .iter()
                .zip(&other.rig_framings)
                .all(|(a, b)| same_rig_framing(a, b))
    }
}

/// One rig's own layout over the shared target.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(try_from = "RigFramingBody")]
pub struct RigFraming {
    pub rig_id: Uuid,
    /// This rig's own target center; None follows the shared center, which
    /// is where every rig starts.
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub center: Option<IcrsPosition>,
    /// This rig's camera angle; None follows the shared angle.
    #[serde(deserialize_with = "Option::deserialize")]
    pub position_angle_degrees: Option<f64>,
    /// This rig's own panels, placed around its center.
    pub layout: Layout,
    /// A panel size set by hand; None means the rig's own field of view.
    #[serde(deserialize_with = "Option::deserialize")]
    pub panel: Option<PanelSize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RigFramingBody {
    rig_id: Uuid,
    #[serde(default, deserialize_with = "Option::deserialize")]
    center: Option<IcrsPosition>,
    #[serde(deserialize_with = "Option::deserialize")]
    position_angle_degrees: Option<f64>,
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    mosaic: Option<Mosaic>,
    #[serde(deserialize_with = "Option::deserialize")]
    panel: Option<PanelSize>,
}

impl TryFrom<RigFramingBody> for RigFraming {
    type Error = String;

    fn try_from(body: RigFramingBody) -> Result<Self, Self::Error> {
        Ok(Self {
            rig_id: body.rig_id,
            center: body.center,
            position_angle_degrees: body.position_angle_degrees,
            layout: layout_or_grid(body.layout, body.mosaic)?,
            panel: body.panel,
        })
    }
}

/// The layout one rig shoots: shared or its own, with every gap filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct RigLayout {
    pub center: IcrsPosition,
    pub position_angle_degrees: f64,
    pub panel: PanelSize,
    pub layout: Layout,
    /// Whether the rig has a framing of its own rather than the shared one.
    pub own: bool,
}

impl RigLayout {
    /// The request the core lays this rig's panels out from.
    pub fn request(&self) -> FramingRequest {
        FramingRequest {
            center: self.center,
            position_angle_degrees: self.position_angle_degrees,
            panel: self.panel,
            layout: self.layout.clone(),
            overlays: vec![],
            view: None,
        }
    }
}

impl FramingDraft {
    pub fn rig_framing(&self, rig: Uuid) -> Option<&RigFraming> {
        self.rig_framings.iter().find(|entry| entry.rig_id == rig)
    }

    /// What `rig` shoots, given its current field of view: its own framing
    /// when it has one (its field standing in for a panel size not set by
    /// hand), else the shared framing. None when no panel size is known.
    pub fn layout_for(&self, rig: Uuid, field: Option<PanelSize>) -> Option<RigLayout> {
        match self.rig_framing(rig) {
            Some(own) => Some(RigLayout {
                center: own.center.unwrap_or(self.center),
                position_angle_degrees: own
                    .position_angle_degrees
                    .unwrap_or(self.position_angle_degrees),
                panel: own.panel.or(field)?,
                layout: own.layout.clone(),
                own: true,
            }),
            None => Some(RigLayout {
                center: self.center,
                position_angle_degrees: self.position_angle_degrees,
                panel: self.panel?,
                layout: self.layout.clone(),
                own: false,
            }),
        }
    }

    /// The shared framing's request, once it has a panel size.
    pub fn shared_request(&self) -> Option<FramingRequest> {
        Some(FramingRequest {
            center: self.center,
            position_angle_degrees: self.position_angle_degrees,
            panel: self.panel?,
            layout: self.layout.clone(),
            overlays: vec![],
            view: None,
        })
    }

    /// The locked panels `next` would move: each panel both drafts mark
    /// locked, whose center or camera angle on the sky differs from where
    /// this draft has it. A lock holds the panel, so the framing's center,
    /// angle and panel size cannot move under it either. Locking and
    /// unlocking are edits of their own: a body may move a panel and lock
    /// it, or unlock it and move it, in one save.
    pub fn locked_moves(&self, next: &Self) -> Vec<String> {
        let mut moved = Vec::new();
        let shared = |draft: &Self| Placing {
            center: draft.center,
            angle: draft.position_angle_degrees,
            panel: draft.panel,
        };
        moved_locks(
            &shared(self),
            &self.layout,
            &shared(next),
            &next.layout,
            "",
            &mut moved,
        );
        for own in &next.rig_framings {
            let Some(before) = self.rig_framing(own.rig_id) else {
                continue;
            };
            let placing = |draft: &Self, own: &RigFraming| Placing {
                center: own.center.unwrap_or(draft.center),
                angle: own
                    .position_angle_degrees
                    .unwrap_or(draft.position_angle_degrees),
                panel: own.panel,
            };
            moved_locks(
                &placing(self, before),
                &before.layout,
                &placing(next, own),
                &own.layout,
                &format!("{}:", own.rig_id),
                &mut moved,
            );
        }
        moved
    }
}

/// Where a framing puts its panels: around a center, turned by an angle,
/// sized by a panel.
struct Placing {
    center: IcrsPosition,
    angle: f64,
    panel: Option<PanelSize>,
}

impl Placing {
    /// A panel's center on the sky. Without a panel size its place is only
    /// its offsets, so the offsets themselves stand in.
    fn sky(&self, x: f64, y: f64) -> Option<IcrsPosition> {
        let panel = self.panel?;
        let plane = TangentPlane::at(self.center).ok()?;
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let (dx, dy) = (x * panel.width_degrees, y * panel.height_degrees);
        Some(plane.deproject([dx * -cos + dy * sin, dx * sin + dy * cos]))
    }
}

fn moved_locks(
    before: &Placing,
    was: &Layout,
    after: &Placing,
    now: &Layout,
    prefix: &str,
    out: &mut Vec<String>,
) {
    for panel in now.panels.iter().filter(|panel| panel.locked) {
        let Some(old) = was.panel(&panel.id).filter(|old| old.locked) else {
            continue;
        };
        let same_offsets = (old.x - panel.x).abs() <= PLACE_TOLERANCE_PANELS
            && (old.y - panel.y).abs() <= PLACE_TOLERANCE_PANELS;
        // A panel measured from its frames is held where they were shot,
        // whatever the camera the plan uses now: its planned place stays,
        // and a new panel size does not move it.
        let place_held = if panel.measured.is_some() || old.measured.is_some() {
            same_position(&before.center, &after.center) && same_offsets
        } else {
            match (before.sky(old.x, old.y), after.sky(panel.x, panel.y)) {
                (Some(a), Some(b)) => same_position(&a, &b),
                // A size given or taken away moves a panel off the center:
                // its place in degrees is gone, or new.
                (Some(_), None) | (None, Some(_)) => {
                    same_offsets
                        && old.x.abs() <= PLACE_TOLERANCE_PANELS
                        && old.y.abs() <= PLACE_TOLERANCE_PANELS
                        && same_position(&before.center, &after.center)
                }
                (None, None) => same_position(&before.center, &after.center) && same_offsets,
            }
        };
        // Its own turn holds as well: a locked panel's camera angle is
        // the one it was shot at.
        let turn_held = same_angle(old.angle_offset_degrees, panel.angle_offset_degrees);
        if !place_held || !turn_held || !same_angle(before.angle, after.angle) {
            out.push(format!("{prefix}{}", panel.id));
        }
    }
}

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

const MAX_SURVEY_ID_LEN: usize = 256;
/// The view's width is the view's own setting, not part of the layout: as
/// wide as the sky and feasibility endpoints draw, which covers zooming out
/// to see the constellations around a target.
const VIEW_FOV_DEGREES: std::ops::RangeInclusive<f64> = 0.02..=180.0;

pub(crate) fn validate_draft(draft: &FramingDraft) -> Result<(), Error> {
    valid_id(draft.project_id)?;
    if draft.target_name.len() > 256
        || draft.target_name.trim() != draft.target_name
        || draft.target_name.chars().any(char::is_control)
        // A layer is found by exact match, so any printable id will do. The
        // offline N.I.N.A. maps are `nina:` and their folder name.
        || draft.survey_id.is_empty()
        || draft.survey_id.len() > MAX_SURVEY_ID_LEN
        || draft.survey_id.trim() != draft.survey_id
        || draft.survey_id.chars().any(char::is_control)
        || !draft.view_fov_degrees.is_finite()
        || !VIEW_FOV_DEGREES.contains(&draft.view_fov_degrees)
        || draft.shown_rig_ids.len() > 64
        || draft.updated_at_ms > MAX_TIME_MS
    {
        return Err(Error::InvalidInput);
    }
    if let Some(rig) = draft.panel_rig_id {
        valid_id(rig)?;
    }
    for rig in &draft.shown_rig_ids {
        valid_id(*rig)?;
    }
    if draft.rig_framings.len() > 64 {
        return Err(Error::InvalidInput);
    }
    let mut framed = std::collections::BTreeSet::new();
    for own in &draft.rig_framings {
        valid_id(own.rig_id)?;
        if !framed.insert(own.rig_id) {
            return Err(Error::InvalidInput);
        }
        FramingRequest {
            center: own.center.unwrap_or(draft.center),
            position_angle_degrees: own
                .position_angle_degrees
                .unwrap_or(draft.position_angle_degrees),
            panel: own.panel.unwrap_or(PanelSize {
                width_degrees: 1.0,
                height_degrees: 1.0,
            }),
            layout: own.layout.clone(),
            overlays: vec![],
            view: None,
        }
        .validate()
        .map_err(|_| Error::InvalidInput)?;
    }
    // The core validates the geometry; a draft without a panel size still
    // needs a sound center, angle and set of panels.
    let request = FramingRequest {
        center: draft.center,
        position_angle_degrees: draft.position_angle_degrees,
        panel: draft.panel.unwrap_or(PanelSize {
            width_degrees: 1.0,
            height_degrees: 1.0,
        }),
        layout: draft.layout.clone(),
        overlays: vec![],
        view: None,
    };
    request.validate().map_err(|_| Error::InvalidInput)?;
    Ok(())
}

/// Schema 29: rewrite every stored draft's `mosaic` grid as the panels it
/// makes, in place, keeping the revisions. Each panel keeps its grid id
/// and its place; nothing else in the payload changes. A payload that does
/// not read as a draft is left exactly as it was, so a damaged record stays
/// the one record that cannot be read.
pub(crate) fn migrate_grids_to_panels(conn: &Connection) -> Result<usize, Error> {
    let mut statement =
        conn.prepare("SELECT project_id, CAST(payload AS BLOB) FROM framing_draft")?;
    let rows: Vec<(String, Vec<u8>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(statement);
    let mut rewritten = 0;
    for (project, payload) in rows {
        let Some(next) = std::str::from_utf8(&payload)
            .ok()
            .and_then(grid_payload_as_panels)
        else {
            continue;
        };
        conn.execute(
            "UPDATE framing_draft SET payload=?2 WHERE project_id=?1",
            params![project, next],
        )?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// A stored payload with its grids written as panels, or `None` when it has
/// no grid to rewrite or does not read as a draft.
fn grid_payload_as_panels(payload: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object_mut()?;
    let mut changed = rewrite_grid(object)?;
    if let Some(framings) = object
        .get_mut("rig_framings")
        .and_then(serde_json::Value::as_array_mut)
    {
        for own in framings {
            changed |= rewrite_grid(own.as_object_mut()?)?;
        }
    }
    if !changed {
        return None;
    }
    let next = serde_json::to_string(&value).ok()?;
    // Only a payload that reads back as a sound draft replaces the old one.
    let draft: FramingDraft = serde_json::from_str(&next).ok()?;
    validate_draft(&draft).ok()?;
    (next.len() <= psf_guard_director_core::MAX_REQUEST_BYTES).then_some(next)
}

/// Replace one object's `mosaic` with the `layout` that grid makes. `Some`
/// with whether it changed; `None` for a grid that does not read.
fn rewrite_grid(object: &mut serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    let Some(mosaic) = object.get("mosaic") else {
        return Some(false);
    };
    if object.contains_key("layout") {
        return None;
    }
    let mosaic: Mosaic = serde_json::from_value(mosaic.clone()).ok()?;
    mosaic.validate().ok()?;
    let layout = serde_json::to_value(Layout::grid(mosaic)).ok()?;
    object.remove("mosaic");
    object.insert("layout".into(), layout);
    Some(true)
}

impl MetaStore {
    pub fn framing_draft(&self, project: Uuid) -> Result<Option<FramingDraft>, Error> {
        read_draft(&self.connection, project)
    }

    /// Save with compare-and-set. `expected_revision` is the revision the
    /// caller read (0 when none existed). An unchanged body under the right
    /// revision returns the stored record without bumping it.
    pub fn save_framing_draft(
        &mut self,
        draft: &FramingDraft,
        expected_revision: u64,
    ) -> Result<FramingDraft, Error> {
        validate_draft(draft)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, draft.project_id)?.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(rig) = draft.panel_rig_id
            && read_named(&tx, Kind::Rig, rig)?.is_none()
        {
            return Err(Error::NotFound);
        }
        for own in &draft.rig_framings {
            if read_named(&tx, Kind::Rig, own.rig_id)?.is_none() {
                return Err(Error::NotFound);
            }
        }
        let stored = read_draft(&tx, draft.project_id)?;
        let current = stored.as_ref().map_or(0, |value| value.revision);
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        // A locked panel stays where it is until someone unlocks it.
        if stored
            .as_ref()
            .is_some_and(|stored| !stored.locked_moves(draft).is_empty())
        {
            return Err(Error::InvalidInput);
        }
        let mut next = draft.clone();
        next.revision = current;
        next.layout_revision = stored.as_ref().map_or(0, |value| value.layout_revision);
        if let Some(stored) = &stored {
            let mut same = stored.clone();
            same.updated_at_ms = next.updated_at_ms;
            if same == next {
                tx.commit()?;
                return Ok(stored.clone());
            }
        }
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
        if stored
            .as_ref()
            .is_none_or(|stored| !stored.same_layout(&next))
        {
            next.layout_revision = next.revision;
        }
        let payload = super::configuration::encode(&next)?;
        tx.execute(
            "INSERT INTO framing_draft(project_id,revision,payload) VALUES(?1,?2,?3)
             ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision, payload=excluded.payload",
            params![
                next.project_id.to_string(),
                i64::try_from(next.revision).map_err(|_| Error::Conflict)?,
                payload
            ],
        )?;
        tx.commit()?;
        Ok(next)
    }
}

pub(crate) fn read_draft(conn: &Connection, project: Uuid) -> Result<Option<FramingDraft>, Error> {
    valid_id(project)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM framing_draft WHERE project_id=?1",
            params![
                project.to_string(),
                (psf_guard_director_core::MAX_REQUEST_BYTES + 1) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let mut value: FramingDraft = super::configuration::decode(payload)?;
        validate_draft(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.project_id != project
            || revision <= 0
            || value.revision != revision as u64
            || value.layout_revision > value.revision
        {
            return Err(Error::CorruptDatabase);
        }
        // Saved before layouts were tracked: assume its layout is as new as it.
        if value.layout_revision == 0 {
            value.layout_revision = value.revision;
        }
        Ok(value)
    })
    .transpose()
}
