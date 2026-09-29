//! The mutable framing draft of a global project: where it points, how it is
//! turned, and how many panels cover it. A draft is what the operator is
//! still editing; activation later freezes intent from it. It grants nothing.

use super::*;
use psf_guard_director_core::{
    framing::{FramingRequest, Mosaic, PanelSize},
    visibility::IcrsPosition,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FramingDraft {
    pub project_id: Uuid,
    /// Compare-and-set revision; every accepted change increments it.
    pub revision: u64,
    pub target_name: String,
    pub center: IcrsPosition,
    pub position_angle_degrees: f64,
    pub mosaic: Mosaic,
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
    /// Rigs framed on their own over the same target: their own grid, panel
    /// size and camera angle. A rig not listed shoots the shared framing.
    #[serde(default)]
    pub rig_framings: Vec<RigFraming>,
}

/// One rig's own layout over the shared target.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigFraming {
    pub rig_id: Uuid,
    /// This rig's own target center; None follows the shared center, which
    /// is where every rig starts.
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub center: Option<IcrsPosition>,
    /// This rig's camera angle; None follows the shared angle.
    #[serde(deserialize_with = "Option::deserialize")]
    pub position_angle_degrees: Option<f64>,
    pub mosaic: Mosaic,
    /// A panel size set by hand; None means the rig's own field of view.
    #[serde(deserialize_with = "Option::deserialize")]
    pub panel: Option<PanelSize>,
}

/// The layout one rig shoots: shared or its own, with every gap filled in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigLayout {
    pub center: IcrsPosition,
    pub position_angle_degrees: f64,
    pub panel: PanelSize,
    pub mosaic: Mosaic,
    /// Whether the rig has a framing of its own rather than the shared one.
    pub own: bool,
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
                mosaic: own.mosaic,
                own: true,
            }),
            None => Some(RigLayout {
                center: self.center,
                position_angle_degrees: self.position_angle_degrees,
                panel: self.panel?,
                mosaic: self.mosaic,
                own: false,
            }),
        }
    }
}

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01

pub(crate) fn validate_draft(draft: &FramingDraft) -> Result<(), Error> {
    valid_id(draft.project_id)?;
    if draft.target_name.len() > 256
        || draft.target_name.trim() != draft.target_name
        || draft.target_name.chars().any(char::is_control)
        || draft.survey_id.is_empty()
        || draft.survey_id.len() > 64
        || !draft
            .survey_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || !draft.view_fov_degrees.is_finite()
        || !(0.02..=40.0).contains(&draft.view_fov_degrees)
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
            mosaic: own.mosaic,
            overlays: vec![],
            view: None,
        }
        .validate()
        .map_err(|_| Error::InvalidInput)?;
    }
    // The core validates the geometry; a draft without a panel size still
    // needs a sound center, angle and grid.
    let request = FramingRequest {
        center: draft.center,
        position_angle_degrees: draft.position_angle_degrees,
        panel: draft.panel.unwrap_or(PanelSize {
            width_degrees: 1.0,
            height_degrees: 1.0,
        }),
        mosaic: draft.mosaic,
        overlays: vec![],
        view: None,
    };
    request.validate().map_err(|_| Error::InvalidInput)?;
    Ok(())
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
        let mut next = draft.clone();
        next.revision = current;
        if let Some(stored) = stored {
            let mut same = stored.clone();
            same.updated_at_ms = next.updated_at_ms;
            if same == next {
                tx.commit()?;
                return Ok(stored);
            }
        }
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
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
        let value: FramingDraft = super::configuration::decode(payload)?;
        validate_draft(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.project_id != project || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
