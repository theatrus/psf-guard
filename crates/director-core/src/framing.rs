//! Footprint and mosaic geometry for the framing view. Pure arithmetic on a
//! tangent plane around the project's center; the browser only draws what
//! this returns. A planned footprint says where a rig would point; a
//! measured one, where its frames were shot.

use crate::visibility::IcrsPosition;
use serde::{Deserialize, Serialize};

pub mod layout;
pub mod measured;
pub mod shape;
pub use layout::{Layout, LayoutIssue, LayoutIssueKind, PanelPlacement};
pub use measured::{MeasuredGeometry, MeasuredSource};
pub use shape::PanelShape;

pub const FRAMING_VERSION: u32 = 1;
pub const MAX_PANELS: usize = 256;
const MAX_GRID: u32 = 16;
/// Total mosaic extent where the flat tangent-plane layout stays honest.
const MAX_EXTENT_DEGREES: f64 = 30.0;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PanelSize {
    pub width_degrees: f64,
    pub height_degrees: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Mosaic {
    pub rows: u32,
    pub columns: u32,
    /// Overlap between neighbouring panels as a share of the panel, 0 to 90.
    pub overlap_percent: u32,
}

impl Mosaic {
    pub const SINGLE: Self = Self {
        rows: 1,
        columns: 1,
        overlap_percent: 0,
    };
}

/// The image the browser draws over: a tangent plane at `center`, turned so
/// that `rotation_degrees` east of north points up.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub center: IcrsPosition,
    pub rotation_degrees: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(try_from = "FramingRequestBody")]
pub struct FramingRequest {
    pub center: IcrsPosition,
    /// Camera up direction, east of north, shared by every panel.
    pub position_angle_degrees: f64,
    pub panel: PanelSize,
    /// Every panel, each placed on its own.
    pub layout: Layout,
    /// Extra footprints to place at the center, such as other rigs' fields.
    #[serde(default)]
    pub overlays: Vec<Overlay>,
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub view: Option<View>,
}

/// A request as sent: the panels as a `layout`, or, from a client written
/// before panels were placed one by one, as a `mosaic` grid.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FramingRequestBody {
    center: IcrsPosition,
    position_angle_degrees: f64,
    panel: PanelSize,
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    mosaic: Option<Mosaic>,
    #[serde(default)]
    overlays: Vec<Overlay>,
    #[serde(default, deserialize_with = "Option::deserialize")]
    view: Option<View>,
}

impl TryFrom<FramingRequestBody> for FramingRequest {
    type Error = String;

    fn try_from(body: FramingRequestBody) -> Result<Self, Self::Error> {
        Ok(Self {
            center: body.center,
            position_angle_degrees: body.position_angle_degrees,
            panel: body.panel,
            layout: layout_or_grid(body.layout, body.mosaic)?,
            overlays: body.overlays,
            view: body.view,
        })
    }
}

/// The panels a body gives: its `layout`, or the grid its `mosaic` makes.
/// One of the two, never both.
pub fn layout_or_grid(layout: Option<Layout>, mosaic: Option<Mosaic>) -> Result<Layout, String> {
    match (layout, mosaic) {
        (Some(layout), None) => Ok(layout),
        // A grid the core refuses makes no panels, so validation names it
        // as it names any bad layout; it is never laid out at its size.
        (None, Some(mosaic)) if mosaic.validate().is_err() => Ok(Layout {
            min_overlap_percent: mosaic.overlap_percent,
            panels: vec![],
        }),
        (None, Some(mosaic)) => Ok(Layout::grid(mosaic)),
        (Some(_), Some(_)) => Err("send `layout` or `mosaic`, not both".into()),
        (None, None) => Err("missing field `layout`".into()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    pub id: String,
    pub size: PanelSize,
    pub position_angle_degrees: f64,
}

/// Tangent-plane offsets in degrees: `xi` grows to the east, `eta` north.
pub type Offset = [f64; 2];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub id: String,
    pub center: IcrsPosition,
    /// Corners in sky coordinates, counter-clockwise from the top left.
    pub corners: [IcrsPosition; 4],
    /// The same corners as offsets from the view center, already turned so
    /// the view's up is `+eta`. Absent without a view.
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub view_corners: Option<[Offset; 4]>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    /// The row and column the panel falls in, counted from the top and the
    /// camera's left: a grid's own cell, or for a free layout the sparse
    /// grid its panels fall on. For ordering and labels only.
    pub row: u32,
    pub column: u32,
    #[serde(default)]
    pub locked: bool,
    /// Unlocked by hand although its target has frames.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlocked_by_hand: bool,
    /// The camera angle to shoot it at, east of north at its own center:
    /// the framing's angle plus the panel's own turn, turned by north's
    /// lean there ([`camera_angle_at`]), or as shot for a measured panel.
    #[serde(default)]
    pub position_angle_degrees: f64,
    /// The panel's own turn from the framing's angle on the plane.
    #[serde(default)]
    pub angle_offset_degrees: f64,
    /// Where its frames were shot, for a locked panel measured from them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<MeasuredGeometry>,
    #[serde(flatten)]
    pub footprint: Footprint,
}

/// Unit vectors at a sky position: toward it, east and north.
fn basis(position: IcrsPosition) -> [[f64; 3]; 3] {
    let (sa, ca) = position.ra_degrees.to_radians().sin_cos();
    let (sd, cd) = position.dec_degrees.to_radians().sin_cos();
    [
        [cd * ca, cd * sa, sd],
        [-sa, ca, 0.0],
        [-sd * ca, -sd * sa, cd],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// North's lean at `at` on the tangent plane about `center`: the position
/// angle, east of north at `at`, of the direction the gnomonic projection
/// draws parallel to the plane's own north. A camera at `at` turned to
/// `θ + north_turn` has its up along `θ` on the plane, as the framing drew
/// it. Zero on the plane's central meridian; about `Δα · sin δ` elsewhere,
/// so panels 3° apart in right ascension at +60° lean 2.6° apart.
///
/// On the plane the tangent `T` at `P` draws as
/// `(Tx·Pz − Px·Tz, Ty·Pz − Py·Tz)` in the plane's (east, north, out) frame.
/// The tangent drawn straight up is `T ∝ (−Px·Py, Px² + Pz², −Py·Pz)`, and
/// its position angle at `P` is the turn.
pub fn north_turn(center: IcrsPosition, at: IcrsPosition) -> f64 {
    let angle = camera_angle_at(center, at, 0.0);
    (angle + 180.0).rem_euclid(360.0) - 180.0
}

/// The position angle a camera at `at` takes so its up draws at `theta`
/// (east of north) on the tangent plane about `center`: `theta` plus north's
/// lean there ([`north_turn`]) to first order, exactly here. The gnomonic
/// plane does not keep angles, so for `theta` away from 0° and 180° the two
/// differ, by a few hundredths of a degree three degrees from the center.
///
/// A tangent `T` at `P` draws as `A·T = (Tx·Pz − Px·Tz, Ty·Pz − Py·Tz)` in
/// the plane's (east, north, out) frame. It draws along `d` when
/// `T ⊥ w = (Pz·dy, −Pz·dx, Py·dx − Px·dy)`; with `T ⊥ P` too,
/// `T ∝ P × w`, and its position angle at `P` is the answer.
pub fn camera_angle_at(center: IcrsPosition, at: IcrsPosition, theta: f64) -> f64 {
    let [z, x, y] = basis(center);
    let [p, east, north] = basis(at);
    let (px, py, pz) = (dot(p, x), dot(p, y), dot(p, z));
    if pz <= 1e-9 {
        return normalise_angle(theta);
    }
    let (dx, dy) = {
        let (sin, cos) = theta.to_radians().sin_cos();
        (sin, cos)
    };
    let w = [pz * dy, -pz * dx, py * dx - px * dy];
    let p_local = [px, py, pz];
    let mut t = [
        p_local[1] * w[2] - p_local[2] * w[1],
        p_local[2] * w[0] - p_local[0] * w[2],
        p_local[0] * w[1] - p_local[1] * w[0],
    ];
    // Point it along `d`, not against it.
    let drawn = [t[0] * pz - px * t[2], t[1] * pz - py * t[2]];
    if drawn[0] * dx + drawn[1] * dy < 0.0 {
        t = [-t[0], -t[1], -t[2]];
    }
    let tangent = [
        t[0] * x[0] + t[1] * y[0] + t[2] * z[0],
        t[0] * x[1] + t[1] * y[1] + t[2] * z[1],
        t[0] * x[2] + t[1] * y[2] + t[2] * z[2],
    ];
    normalise_angle(dot(tangent, east).atan2(dot(tangent, north)).to_degrees())
}

/// The angle on the tangent plane about `center` that a camera at `at`
/// shooting at `position_angle` draws its up along: the inverse of
/// [`camera_angle_at`], so a target's stored angle reads back as the
/// framing's.
pub fn plane_angle_at(center: IcrsPosition, at: IcrsPosition, position_angle: f64) -> f64 {
    let wrapped = |degrees: f64| (degrees + 180.0).rem_euclid(360.0) - 180.0;
    let miss = |theta: f64| wrapped(position_angle - camera_angle_at(center, at, theta));
    let mut theta = position_angle - north_turn(center, at);
    // Newton's method on the angle the camera draws, its slope read a
    // hair either side: near the center the map is the identity plus a
    // small term and one step settles it; far out its slope is well away
    // from one, and a plain fixed point would crawl. Stop at rounding.
    for _ in 0..50 {
        let error = miss(theta);
        if error.abs() < 1e-11 {
            break;
        }
        const STEP: f64 = 1e-4;
        let slope = (miss(theta - STEP) - miss(theta + STEP)) / (2.0 * STEP);
        theta += if slope.is_finite() && slope.abs() > 1e-6 {
            error / slope
        } else {
            error
        };
    }
    normalise_angle(theta)
}

/// An angle in [0, 360), rounded to a billionth of a degree.
fn normalise_angle(degrees: f64) -> f64 {
    let rounded = (degrees.rem_euclid(360.0) * 1e9).round() / 1e9;
    if rounded >= 360.0 {
        0.0
    } else {
        rounded
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FramingPreview {
    pub schema_version: u32,
    pub panels: Vec<Panel>,
    pub overlays: Vec<Footprint>,
    /// Extent of the whole mosaic along the camera axes.
    pub extent: PanelSize,
    /// Where the view center sits, for the browser's crosshair.
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub view_center_offset: Option<Offset>,
    /// Where the panels fall short of the minimum overlap rule.
    #[serde(default)]
    pub issues: Vec<LayoutIssue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FramingError {
    InvalidCenter,
    InvalidAngle,
    InvalidPanel,
    InvalidMosaic,
    TooLarge,
    InvalidOverlay,
    OutsideView,
}

fn finite(value: f64, minimum: f64, maximum: f64) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

fn valid_position(position: &IcrsPosition) -> bool {
    finite(position.ra_degrees, 0.0, 360.0)
        && position.ra_degrees < 360.0
        && finite(position.dec_degrees, -90.0, 90.0)
}

fn valid_angle(angle: f64) -> bool {
    finite(angle, 0.0, 360.0) && angle < 360.0
}

impl PanelSize {
    pub fn validate(&self) -> Result<(), FramingError> {
        if !finite(self.width_degrees, 0.01, MAX_EXTENT_DEGREES)
            || !finite(self.height_degrees, 0.01, MAX_EXTENT_DEGREES)
        {
            return Err(FramingError::InvalidPanel);
        }
        Ok(())
    }
}

impl Mosaic {
    pub fn validate(&self) -> Result<(), FramingError> {
        if !(1..=MAX_GRID).contains(&self.rows)
            || !(1..=MAX_GRID).contains(&self.columns)
            || self.overlap_percent > 90
            || (self.rows as usize) * (self.columns as usize) > MAX_PANELS
        {
            return Err(FramingError::InvalidMosaic);
        }
        Ok(())
    }
}

/// Gnomonic projection about one center. Offsets are true-angle degrees on
/// the tangent plane, exact for any separation short of the antipode.
#[derive(Clone, Copy, Debug)]
pub struct TangentPlane {
    ra0: f64,
    sin_dec0: f64,
    cos_dec0: f64,
}

impl TangentPlane {
    pub fn at(center: IcrsPosition) -> Result<Self, FramingError> {
        if !valid_position(&center) {
            return Err(FramingError::InvalidCenter);
        }
        let dec0 = center.dec_degrees.to_radians();
        Ok(Self {
            ra0: center.ra_degrees.to_radians(),
            sin_dec0: dec0.sin(),
            cos_dec0: dec0.cos(),
        })
    }

    /// `None` for points on or beyond the horizon of the plane.
    pub fn project(&self, position: IcrsPosition) -> Option<Offset> {
        let ra = position.ra_degrees.to_radians();
        let dec = position.dec_degrees.to_radians();
        let (sin_dec, cos_dec) = dec.sin_cos();
        let (sin_dra, cos_dra) = (ra - self.ra0).sin_cos();
        let cos_c = self.sin_dec0 * sin_dec + self.cos_dec0 * cos_dec * cos_dra;
        if cos_c.is_nan() || cos_c <= 1e-9 {
            return None;
        }
        let xi = cos_dec * sin_dra / cos_c;
        let eta = (self.cos_dec0 * sin_dec - self.sin_dec0 * cos_dec * cos_dra) / cos_c;
        Some([xi.to_degrees(), eta.to_degrees()])
    }

    pub fn deproject(&self, offset: Offset) -> IcrsPosition {
        let xi = offset[0].to_radians();
        let eta = offset[1].to_radians();
        let denominator = self.cos_dec0 - eta * self.sin_dec0;
        let ra = self.ra0 + xi.atan2(denominator);
        let dec = ((self.sin_dec0 + eta * self.cos_dec0) / (1.0 + xi * xi + eta * eta).sqrt())
            .clamp(-1.0, 1.0)
            .asin();
        IcrsPosition {
            ra_degrees: ra.to_degrees().rem_euclid(360.0),
            dec_degrees: dec.to_degrees(),
        }
    }
}

/// Rectangle corners on the plane: counter-clockwise from the top left, with
/// "up" at `angle_degrees` east of north.
fn rectangle(center: Offset, size: PanelSize, angle_degrees: f64) -> [Offset; 4] {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    // Up points toward the angle; right is up turned a quarter turn clockwise,
    // which on an east-positive plane is toward the west at angle zero.
    let up = [sin, cos];
    let right = [-cos, sin];
    let (hw, hh) = (size.width_degrees / 2.0, size.height_degrees / 2.0);
    let corner = |sx: f64, sy: f64| {
        [
            center[0] + sx * hw * right[0] + sy * hh * up[0],
            center[1] + sx * hw * right[1] + sy * hh * up[1],
        ]
    };
    [
        corner(-1.0, 1.0),
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
    ]
}

fn turn(offset: Offset, degrees: f64) -> Offset {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [
        offset[0] * cos - offset[1] * sin,
        offset[0] * sin + offset[1] * cos,
    ]
}

impl FramingRequest {
    pub fn validate(&self) -> Result<(), FramingError> {
        if !valid_position(&self.center) {
            return Err(FramingError::InvalidCenter);
        }
        if !valid_angle(self.position_angle_degrees) {
            return Err(FramingError::InvalidAngle);
        }
        self.panel.validate()?;
        self.layout.validate()?;
        // Every panel within the reach where the flat layout stays honest,
        // a measured one by where it was shot.
        let plane = TangentPlane::at(self.center)?;
        for panel in &self.layout.panels {
            let (dx, dy) = match &panel.measured {
                Some(measured) => {
                    let offset = plane
                        .project(measured.center)
                        .ok_or(FramingError::TooLarge)?;
                    (offset[0], offset[1])
                }
                None => (
                    panel.x * self.panel.width_degrees,
                    panel.y * self.panel.height_degrees,
                ),
            };
            if dx.abs() > MAX_EXTENT_DEGREES || dy.abs() > MAX_EXTENT_DEGREES {
                return Err(FramingError::TooLarge);
            }
        }
        let extent = self.extent();
        if extent.width_degrees > MAX_EXTENT_DEGREES || extent.height_degrees > MAX_EXTENT_DEGREES {
            return Err(FramingError::TooLarge);
        }
        if self.overlays.len() > 64 {
            return Err(FramingError::InvalidOverlay);
        }
        for overlay in &self.overlays {
            if overlay.id.is_empty()
                || overlay.id.len() > 128
                || overlay.id.chars().any(char::is_control)
            {
                return Err(FramingError::InvalidOverlay);
            }
            overlay
                .size
                .validate()
                .map_err(|_| FramingError::InvalidOverlay)?;
            if !valid_angle(overlay.position_angle_degrees) {
                return Err(FramingError::InvalidOverlay);
            }
        }
        if let Some(view) = &self.view
            && (!valid_position(&view.center) || !valid_angle(view.rotation_degrees))
        {
            return Err(FramingError::InvalidCenter);
        }
        Ok(())
    }

    /// The whole mosaic along the camera axes, edge to edge: planned
    /// panels at their places, measured ones as shot.
    pub fn extent(&self) -> PanelSize {
        let Ok(shapes) = self.shapes() else {
            let (across, up) = self.layout.span();
            return PanelSize {
                width_degrees: self.panel.width_degrees * across,
                height_degrees: self.panel.height_degrees * up,
            };
        };
        let mut b = [
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for shape in &shapes {
            let s = shape.bounds();
            b = [
                b[0].min(s[0]),
                b[1].max(s[1]),
                b[2].min(s[2]),
                b[3].max(s[3]),
            ];
        }
        if shapes.is_empty() {
            return PanelSize {
                width_degrees: 0.0,
                height_degrees: 0.0,
            };
        }
        PanelSize {
            width_degrees: b[1] - b[0],
            height_degrees: b[3] - b[2],
        }
    }

    /// A plane offset (east, north) as the camera frame (right, up).
    fn camera_place(&self, offset: Offset) -> Offset {
        let (sin, cos) = self.position_angle_degrees.to_radians().sin_cos();
        [
            -offset[0] * cos + offset[1] * sin,
            offset[0] * sin + offset[1] * cos,
        ]
    }

    /// A camera-frame place (right, up) as a plane offset (east, north).
    fn plane_offset(&self, place: Offset) -> Offset {
        // The same reflection both ways.
        self.camera_place(place)
    }

    /// A measured panel's corners on the sky: the rectangle its camera
    /// covered on the plane about its own center, at the angle it was shot.
    fn measured_corners(measured: &MeasuredGeometry) -> Result<[IcrsPosition; 4], FramingError> {
        let own = TangentPlane::at(measured.center)?;
        Ok(
            rectangle([0.0, 0.0], measured.size, measured.position_angle_degrees)
                .map(|corner| own.deproject(corner)),
        )
    }

    /// Every panel's outline in the camera frame, in degrees: planned panels
    /// as rectangles at their places, measured ones as shot, projected onto
    /// the framing's plane.
    pub fn shapes(&self) -> Result<Vec<PanelShape>, FramingError> {
        let plane = TangentPlane::at(self.center)?;
        self.layout
            .panels
            .iter()
            .map(|panel| match &panel.measured {
                Some(measured) => {
                    let corners = Self::measured_corners(measured)?;
                    let mut out = [[0.0; 2]; 4];
                    for (slot, corner) in out.iter_mut().zip(corners) {
                        *slot =
                            self.camera_place(plane.project(corner).ok_or(FramingError::TooLarge)?);
                    }
                    Ok(PanelShape {
                        id: panel.id.clone(),
                        corners: out,
                    })
                }
                None => Ok(PanelShape::turned(
                    panel.id.clone(),
                    [
                        panel.x * self.panel.width_degrees,
                        panel.y * self.panel.height_degrees,
                    ],
                    self.panel.width_degrees,
                    self.panel.height_degrees,
                    panel.angle_offset_degrees,
                )),
            })
            .collect()
    }

    /// Where the panels fall short of the minimum overlap rule, read from
    /// their outlines ([`shape::shape_issues`]).
    pub fn issues(&self) -> Result<Vec<LayoutIssue>, FramingError> {
        Ok(shape::shape_issues(
            &self.shapes()?,
            self.layout.min_overlap_percent,
        ))
    }

    /// Every panel's corners on the sky, in the order of [`Self::shapes`].
    fn sky_corners(&self, shapes: &[PanelShape]) -> Result<Vec<[IcrsPosition; 4]>, FramingError> {
        let plane = TangentPlane::at(self.center)?;
        Ok(shapes
            .iter()
            .map(|shape| shape.corners.map(|c| plane.deproject(self.plane_offset(c))))
            .collect())
    }

    pub fn preview(&self) -> Result<FramingPreview, FramingError> {
        self.validate()?;
        let plane = TangentPlane::at(self.center)?;
        let view = self
            .view
            .map(|view| TangentPlane::at(view.center).map(|plane| (plane, view.rotation_degrees)))
            .transpose()?;
        let to_view = |position: IcrsPosition| -> Result<Offset, FramingError> {
            let (plane, rotation) = view.as_ref().expect("checked by caller");
            plane
                .project(position)
                .map(|offset| turn(offset, *rotation))
                .ok_or(FramingError::OutsideView)
        };
        let view_corners =
            |corners: [IcrsPosition; 4]| -> Result<Option<[Offset; 4]>, FramingError> {
                if view.is_none() {
                    return Ok(None);
                }
                let mut mapped = [[0.0; 2]; 4];
                for (slot, corner) in mapped.iter_mut().zip(corners) {
                    *slot = to_view(corner)?;
                }
                Ok(Some(mapped))
            };
        let footprint = |id: String, center: Offset, size: PanelSize, angle: f64| {
            let corners = rectangle(center, size, angle).map(|c| plane.deproject(c));
            Ok::<_, FramingError>(Footprint {
                id,
                center: plane.deproject(center),
                corners,
                view_corners: view_corners(corners)?,
            })
        };
        let shapes = self.shapes()?;
        // Rows and columns from where each panel sits, in panel sizes.
        let places: Vec<[f64; 2]> = shapes
            .iter()
            .map(|shape| {
                let c = shape.center();
                [
                    c[0] / self.panel.width_degrees,
                    c[1] / self.panel.height_degrees,
                ]
            })
            .collect();
        let cells = layout::cells_at(&places);
        let mut panels = Vec::with_capacity(self.layout.panels.len());
        for ((placement, (row, column)), shape) in self.layout.panels.iter().zip(cells).zip(&shapes)
        {
            let footprint = match &placement.measured {
                Some(measured) => {
                    let corners = Self::measured_corners(measured)?;
                    Footprint {
                        id: placement.id.clone(),
                        center: measured.center,
                        corners,
                        view_corners: view_corners(corners)?,
                    }
                }
                None => footprint(
                    placement.id.clone(),
                    self.plane_offset(shape.center()),
                    self.panel,
                    self.position_angle_degrees + placement.angle_offset_degrees,
                )?,
            };
            // The angle a camera at the panel's center takes so its up runs
            // along the panel's angle on the plane; as shot when measured.
            let position_angle_degrees = match &placement.measured {
                Some(measured) => measured.position_angle_degrees,
                None => camera_angle_at(
                    self.center,
                    footprint.center,
                    self.position_angle_degrees + placement.angle_offset_degrees,
                ),
            };
            panels.push(Panel {
                row,
                column,
                locked: placement.locked,
                unlocked_by_hand: placement.unlocked_by_hand,
                position_angle_degrees,
                angle_offset_degrees: placement.angle_offset_degrees,
                measured: placement.measured.clone(),
                footprint,
            });
        }
        let overlays = self
            .overlays
            .iter()
            .map(|overlay| {
                footprint(
                    overlay.id.clone(),
                    [0.0, 0.0],
                    overlay.size,
                    overlay.position_angle_degrees,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let view_center_offset = if view.is_some() {
            Some(to_view(self.center)?)
        } else {
            None
        };
        Ok(FramingPreview {
            schema_version: FRAMING_VERSION,
            panels,
            overlays,
            extent: self.extent(),
            view_center_offset,
            issues: shape::shape_issues(&shapes, self.layout.min_overlap_percent),
        })
    }
}

/// One panel among several framings of one object: the framing's place in
/// the list read, and the panel's id in that framing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CoveragePanel {
    pub framing: usize,
    pub id: String,
}

/// A [`LayoutIssue`] read across framings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CoverageIssue {
    pub kind: LayoutIssueKind,
    pub panels: Vec<CoveragePanel>,
    #[serde(default, deserialize_with = "Option::deserialize")]
    pub percent: Option<f64>,
}

/// Where several framings of one object, such as the shared framing and the
/// rigs framed on their own, fall short as one cover of the sky. A thin
/// overlap is said only where two panels of one framing meet, each framing
/// read as [`FramingRequest::issues`] reads it. A gap, a lone panel and
/// groups apart are read over every panel: a framing's own panels need not
/// touch when another framing's panels fill the sky between them, and two
/// framings' panels that overlap or touch are joined. One framing reads
/// exactly as its own issues.
pub fn coverage_issues(framings: &[FramingRequest]) -> Result<Vec<CoverageIssue>, FramingError> {
    let own = framings
        .iter()
        .map(FramingRequest::shapes)
        .collect::<Result<Vec<_>, _>>()?;
    let sky = framings
        .iter()
        .zip(&own)
        .map(|(request, shapes)| request.sky_corners(shapes))
        .collect::<Result<Vec<_>, _>>()?;
    // Every panel in each framing's camera frame: its own panels as they
    // are, the others' taken there through the sky.
    let mut frames: Vec<Vec<PanelShape>> = Vec::with_capacity(framings.len());
    for (f, request) in framings.iter().enumerate() {
        let plane = TangentPlane::at(request.center)?;
        let mut all = Vec::new();
        for (g, shapes) in own.iter().enumerate() {
            if g == f {
                all.extend(shapes.iter().cloned());
                continue;
            }
            for (shape, corners) in shapes.iter().zip(&sky[g]) {
                let mut out = [[0.0; 2]; 4];
                for (slot, corner) in out.iter_mut().zip(corners) {
                    *slot =
                        request.camera_place(plane.project(*corner).ok_or(FramingError::TooLarge)?);
                }
                all.push(PanelShape {
                    id: shape.id.clone(),
                    corners: out,
                });
            }
        }
        frames.push(all);
    }
    let framing: Vec<usize> = own
        .iter()
        .enumerate()
        .flat_map(|(f, shapes)| std::iter::repeat_n(f, shapes.len()))
        .collect();
    let minimum: Vec<u32> = framings
        .iter()
        .map(|request| request.layout.min_overlap_percent)
        .collect();
    let views: Vec<&[PanelShape]> = frames.iter().map(Vec::as_slice).collect();
    let all = frames.first().map(Vec::as_slice).unwrap_or_default();
    Ok(shape::tagged_issues(&views, &framing, &minimum)
        .into_iter()
        .map(|issue| CoverageIssue {
            kind: issue.kind,
            panels: issue
                .panels
                .iter()
                .map(|&k| CoveragePanel {
                    framing: framing[k],
                    id: all[k].id.clone(),
                })
                .collect(),
            percent: issue.percent,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn request() -> FramingRequest {
        FramingRequest {
            center: IcrsPosition {
                ra_degrees: 10.6847,
                dec_degrees: 41.269,
            },
            position_angle_degrees: 0.0,
            panel: PanelSize {
                width_degrees: 2.0,
                height_degrees: 1.5,
            },
            layout: Layout::grid(Mosaic::SINGLE),
            overlays: vec![],
            view: None,
        }
    }

    #[test]
    fn projection_round_trips_and_keeps_east_positive() {
        let plane = TangentPlane::at(IcrsPosition {
            ra_degrees: 10.0,
            dec_degrees: 40.0,
        })
        .unwrap();
        let east = IcrsPosition {
            ra_degrees: 11.0,
            dec_degrees: 40.0,
        };
        let [xi, eta] = plane.project(east).unwrap();
        assert!(
            xi > 0.0 && close(eta.abs().max(0.0), eta.abs()),
            "{xi} {eta}"
        );
        let back = plane.deproject([xi, eta]);
        assert!(
            close(back.ra_degrees, east.ra_degrees) && close(back.dec_degrees, east.dec_degrees)
        );
        // One degree of true separation on the plane, not one degree of RA.
        assert!((xi - 1.0 * 40f64.to_radians().cos()).abs() < 0.01);
        assert!(plane
            .project(IcrsPosition {
                ra_degrees: 190.0,
                dec_degrees: -40.0
            })
            .is_none());
    }

    #[test]
    fn a_single_panel_at_zero_angle_has_north_up_and_the_stated_size() {
        let preview = request().preview().unwrap();
        assert_eq!(preview.panels.len(), 1);
        let panel = &preview.panels[0];
        assert_eq!(panel.footprint.id, "r1c1");
        let plane = TangentPlane::at(request().center).unwrap();
        let offsets = panel.footprint.corners.map(|c| plane.project(c).unwrap());
        // Top left, bottom left, bottom right, top right; east is to the left.
        assert!(
            close(offsets[0][0], 1.0) && close(offsets[0][1], 0.75),
            "{offsets:?}"
        );
        assert!(close(offsets[1][0], 1.0) && close(offsets[1][1], -0.75));
        assert!(close(offsets[2][0], -1.0) && close(offsets[2][1], -0.75));
        assert!(close(offsets[3][0], -1.0) && close(offsets[3][1], 0.75));
        assert!(
            close(preview.extent.width_degrees, 2.0) && close(preview.extent.height_degrees, 1.5)
        );
        assert!(panel.footprint.corners[0].dec_degrees > panel.footprint.corners[1].dec_degrees);
    }

    #[test]
    fn a_two_by_two_mosaic_steps_by_the_unoverlapped_share() {
        let mut request = request();
        request.layout = Layout::grid(Mosaic {
            rows: 2,
            columns: 2,
            overlap_percent: 20,
        });
        let preview = request.preview().unwrap();
        assert_eq!(preview.panels.len(), 4);
        let plane = TangentPlane::at(request.center).unwrap();
        let centers: Vec<Offset> = preview
            .panels
            .iter()
            .map(|p| plane.project(p.footprint.center).unwrap())
            .collect();
        // Step is 80% of the panel; row one is up, column one is east (left).
        assert!(
            close(centers[0][0], 0.8) && close(centers[0][1], 0.6),
            "{centers:?}"
        );
        assert!(close(centers[1][0], -0.8) && close(centers[1][1], 0.6));
        assert!(close(centers[2][0], 0.8) && close(centers[2][1], -0.6));
        assert!(close(centers[3][0], -0.8) && close(centers[3][1], -0.6));
        assert_eq!(preview.panels[3].footprint.id, "r2c2");
        assert!(
            close(preview.extent.width_degrees, 3.6) && close(preview.extent.height_degrees, 2.7)
        );
    }

    #[test]
    fn a_turned_view_reports_corners_in_screen_terms() {
        let mut request = request();
        request.position_angle_degrees = 90.0;
        request.view = Some(View {
            center: request.center,
            rotation_degrees: 90.0,
        });
        let preview = request.preview().unwrap();
        let corners = preview.panels[0].footprint.view_corners.unwrap();
        // Turning the view with the camera puts the panel back upright on screen.
        assert!(
            close(corners[0][0], 1.0) && close(corners[0][1], 0.75),
            "{corners:?}"
        );
        assert!(close(corners[2][0], -1.0) && close(corners[2][1], -0.75));
        assert_eq!(preview.view_center_offset, Some([0.0, 0.0]));
        request.view = Some(View {
            center: IcrsPosition {
                ra_degrees: 190.0,
                dec_degrees: -41.0,
            },
            rotation_degrees: 0.0,
        });
        assert_eq!(request.preview(), Err(FramingError::OutsideView));
    }

    #[test]
    fn overlays_sit_on_the_center_and_bad_input_is_named() {
        let mut request = request();
        request.overlays.push(Overlay {
            id: "rig-a".into(),
            size: PanelSize {
                width_degrees: 5.0,
                height_degrees: 3.0,
            },
            position_angle_degrees: 45.0,
        });
        let preview = request.preview().unwrap();
        assert_eq!(preview.overlays.len(), 1);
        assert!(close(
            preview.overlays[0].center.ra_degrees,
            request.center.ra_degrees
        ));
        request.layout = Layout::grid(Mosaic {
            rows: 16,
            columns: 16,
            overlap_percent: 0,
        });
        assert_eq!(request.preview(), Err(FramingError::TooLarge));
        request.layout = Layout::grid(Mosaic {
            rows: 0,
            columns: 1,
            overlap_percent: 0,
        });
        assert_eq!(request.preview(), Err(FramingError::InvalidMosaic));
        request.layout = Layout::grid(Mosaic::SINGLE);
        request.position_angle_degrees = 360.0;
        assert_eq!(request.preview(), Err(FramingError::InvalidAngle));
        request.position_angle_degrees = 0.0;
        request.center.dec_degrees = 91.0;
        assert_eq!(request.preview(), Err(FramingError::InvalidCenter));
    }

    #[test]
    fn a_panel_near_the_pole_stays_finite() {
        let mut request = request();
        request.center = IcrsPosition {
            ra_degrees: 0.0,
            dec_degrees: 89.5,
        };
        let preview = request.preview().unwrap();
        for corner in preview.panels[0].footprint.corners {
            assert!(
                corner.dec_degrees <= 90.0 && corner.dec_degrees > 88.0,
                "{corner:?}"
            );
            assert!(corner.ra_degrees.is_finite());
        }
    }

    /// The grid as the core laid it out before panels were placed one by
    /// one: the step is the unoverlapped share of the panel, row one at the
    /// top and column one at the camera's left.
    fn old_grid_centers(request: &FramingRequest, mosaic: Mosaic) -> Vec<(String, IcrsPosition)> {
        let plane = TangentPlane::at(request.center).unwrap();
        let keep = 1.0 - f64::from(mosaic.overlap_percent) / 100.0;
        let (step_x, step_y) = (
            request.panel.width_degrees * keep,
            request.panel.height_degrees * keep,
        );
        let (sin, cos) = request.position_angle_degrees.to_radians().sin_cos();
        let (up, right) = ([sin, cos], [-cos, sin]);
        let mut out = Vec::new();
        for row in 0..mosaic.rows {
            let dy = (f64::from(mosaic.rows - 1) / 2.0 - f64::from(row)) * step_y;
            for column in 0..mosaic.columns {
                let dx = (f64::from(column) - f64::from(mosaic.columns - 1) / 2.0) * step_x;
                out.push((
                    format!("r{}c{}", row + 1, column + 1),
                    plane.deproject([dx * right[0] + dy * up[0], dx * right[1] + dy * up[1]]),
                ));
            }
        }
        out
    }

    #[test]
    fn a_grid_made_into_panels_keeps_every_panel_where_it_was() {
        for (rows, columns, overlap, angle, dec) in [
            (1, 1, 20, 0.0, 41.0),
            (2, 2, 20, 35.0, 61.45),
            (3, 5, 15, 271.5, -72.0),
            (16, 1, 0, 90.0, 10.0),
        ] {
            let mosaic = Mosaic {
                rows,
                columns,
                overlap_percent: overlap,
            };
            let mut request = request();
            request.center.dec_degrees = dec;
            request.position_angle_degrees = angle;
            request.panel = PanelSize {
                width_degrees: 1.2,
                height_degrees: 0.9,
            };
            request.layout = Layout::grid(mosaic);
            let preview = request.preview().unwrap();
            let old = old_grid_centers(&request, mosaic);
            assert_eq!(preview.panels.len(), old.len());
            for (panel, (id, center)) in preview.panels.iter().zip(&old) {
                assert_eq!(&panel.footprint.id, id);
                assert!(
                    (panel.footprint.center.ra_degrees - center.ra_degrees).abs() < 1e-10
                        && (panel.footprint.center.dec_degrees - center.dec_degrees).abs() < 1e-10,
                    "{id} moved: {:?} against {center:?}",
                    panel.footprint.center
                );
                assert_eq!(grid_cell_of(id), (panel.row, panel.column));
            }
            let extent = request.extent();
            let keep = 1.0 - f64::from(overlap) / 100.0;
            assert!(close(
                extent.width_degrees,
                1.2 + 1.2 * keep * f64::from(columns - 1)
            ));
        }
    }

    fn grid_cell_of(id: &str) -> (u32, u32) {
        layout::grid_cell(id).unwrap()
    }

    #[test]
    fn an_added_panel_sits_where_its_offset_says_at_any_angle() {
        let mut request = request();
        request
            .layout
            .panels
            .push(PanelPlacement::at("p2", -0.8, 0.0));
        request.layout.panels[0].locked = true;
        let preview = request.preview().unwrap();
        let plane = TangentPlane::at(request.center).unwrap();
        // At angle zero the camera's left is east, so -0.8 widths is east.
        let at = plane.project(preview.panels[1].footprint.center).unwrap();
        assert!(close(at[0], 1.6) && close(at[1], 0.0), "{at:?}");
        assert!(preview.panels[0].locked && !preview.panels[1].locked);
        assert_eq!(
            (preview.panels[1].row, preview.panels[1].column),
            (1, 1),
            "the eastern panel lists first"
        );
        assert!(preview.issues.is_empty(), "{:?}", preview.issues);
        assert!(close(preview.extent.width_degrees, 3.6));
    }

    /// Overlap is read in the camera frame, so it is the same at every
    /// camera angle: two panels side by side at the minimum share exactly
    /// the minimum of a side on the sky, whichever way the mosaic is turned.
    #[test]
    fn overlap_on_the_sky_follows_the_camera_at_any_angle() {
        for angle in [0.0, 12.5, 37.0, 90.0, 133.0, 225.0, 359.0] {
            let mut request = request();
            request.position_angle_degrees = angle;
            request.layout = Layout {
                min_overlap_percent: 20,
                panels: vec![
                    PanelPlacement::at("r1c1", 0.0, 0.0),
                    PanelPlacement::at("p2", 0.8, 0.0),
                    PanelPlacement::at("p3", 0.0, 0.8),
                ],
            };
            let preview = request.preview().unwrap();
            assert!(preview.issues.is_empty(), "{angle}: {:?}", preview.issues);
            let plane = TangentPlane::at(request.center).unwrap();
            let (sin, cos) = angle.to_radians().sin_cos();
            // Corners on the plane, turned back into the camera frame.
            let camera = |corner: IcrsPosition| {
                let [xi, eta] = plane.project(corner).unwrap();
                [-xi * cos + eta * sin, xi * sin + eta * cos]
            };
            let bounds = |panel: &Panel| {
                let points = panel.footprint.corners.map(camera);
                let xs = points.map(|p| p[0]);
                let ys = points.map(|p| p[1]);
                (
                    xs.iter().cloned().fold(f64::INFINITY, f64::min),
                    xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                    ys.iter().cloned().fold(f64::INFINITY, f64::min),
                    ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                )
            };
            let (_, right_of_first, _, top_of_first) = bounds(&preview.panels[0]);
            let (left_of_second, _, _, _) = bounds(&preview.panels[1]);
            let (_, _, bottom_of_third, _) = bounds(&preview.panels[2]);
            assert!(
                close(right_of_first - left_of_second, 0.4),
                "{angle}: 20% of a 2° width"
            );
            assert!(
                close(top_of_first - bottom_of_third, 0.3),
                "{angle}: 20% of a 1.5° height"
            );
        }
    }

    #[test]
    fn an_older_client_may_send_a_grid_but_not_both() {
        let body = serde_json::json!({
            "center": { "ra_degrees": 10.0, "dec_degrees": 20.0 },
            "position_angle_degrees": 0.0,
            "panel": { "width_degrees": 1.0, "height_degrees": 1.0 },
            "mosaic": { "rows": 1, "columns": 2, "overlap_percent": 10 },
        });
        let request: FramingRequest = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(
            request.layout.as_grid(),
            Some(Mosaic {
                rows: 1,
                columns: 2,
                overlap_percent: 10
            })
        );
        let mut both = body.clone();
        both["layout"] = serde_json::to_value(&request.layout).unwrap();
        assert!(serde_json::from_value::<FramingRequest>(both).is_err());
        let mut neither = body;
        neither.as_object_mut().unwrap().remove("mosaic");
        assert!(serde_json::from_value::<FramingRequest>(neither).is_err());
        // What is written back is the layout.
        let written = serde_json::to_value(&request).unwrap();
        assert!(written.get("layout").is_some() && written.get("mosaic").is_none());
    }

    fn at(ra: f64, dec: f64) -> IcrsPosition {
        IcrsPosition {
            ra_degrees: ra,
            dec_degrees: dec,
        }
    }

    /// The position angle at `from` of the great circle toward `to`.
    fn bearing(from: IcrsPosition, to: IcrsPosition) -> f64 {
        let (d1, d2) = (from.dec_degrees.to_radians(), to.dec_degrees.to_radians());
        let da = (to.ra_degrees - from.ra_degrees).to_radians();
        (da.sin() * d2.cos())
            .atan2(d1.cos() * d2.sin() - d1.sin() * d2.cos() * da.cos())
            .to_degrees()
    }

    /// The worked case: panels 3° apart in right ascension at +60°. North
    /// leans by about Δα·sin δ = 2.598° at the outer panel, and the exact
    /// turn matches the bearing of a short step up the plane from it.
    #[test]
    fn north_leans_by_about_the_right_ascension_apart_times_sin_dec() {
        let center = at(100.0, 60.0);
        assert_eq!(north_turn(center, center), 0.0);
        let east = at(103.0, 60.0);
        let turn = north_turn(center, east);
        assert!(
            (turn - 3.0 * 60f64.to_radians().sin()).abs() < 0.01,
            "{turn}"
        );
        assert!(
            turn > 0.0,
            "east of center, the plane's north is east of local north"
        );
        assert!((north_turn(center, at(97.0, 60.0)) + turn).abs() < 1e-9);
        // On the central meridian north does not lean, whatever the dec.
        assert!(north_turn(center, at(100.0, 63.0)).abs() < 1e-12);
        // South of the equator it leans the other way.
        assert!(north_turn(at(100.0, -60.0), at(103.0, -60.0)) < 0.0);
        // The exact turn is the bearing of a short step up the plane.
        let plane = TangentPlane::at(center).unwrap();
        for target in [east, at(104.5, 58.0), at(96.0, 63.5), at(100.5, 75.0)] {
            let offset = plane.project(target).unwrap();
            let step = plane.deproject([offset[0], offset[1] + 1e-6]);
            let expected = bearing(target, step);
            let turn = north_turn(center, target);
            assert!(
                (turn - expected).abs() < 1e-5,
                "{target:?}: {turn} against {expected}"
            );
        }
    }

    /// A panel shot at its camera angle has its up along θ on the mosaic's
    /// plane: the line through the middles of its top and bottom edges, a
    /// great circle, draws at θ exactly. The angle is θ plus north's lean to
    /// within the plane's angle distortion.
    #[test]
    fn a_stored_angle_reads_back_as_the_framings() {
        let center = at(100.0, 60.0);
        for (place, theta) in [
            (at(103.0, 60.0), 20.0),
            (at(97.5, 61.5), 300.0),
            (center, 45.0),
        ] {
            let shot = camera_angle_at(center, place, theta);
            let back = plane_angle_at(center, place, shot);
            let apart = (back - theta + 180.0).rem_euclid(360.0) - 180.0;
            assert!(apart.abs() < 1e-7, "{theta}: {back}");
        }
    }

    #[test]
    fn a_panel_turned_by_north_lean_lines_up_with_the_plane() {
        let center = at(100.0, 60.0);
        let plane = TangentPlane::at(center).unwrap();
        for theta in [0.0, 30.0, 90.0, 147.0] {
            for target in [at(103.0, 60.0), at(98.0, 61.5), at(101.0, 58.5)] {
                let pa = camera_angle_at(center, target, theta);
                // θ plus north's lean, to the plane's angle distortion.
                let first_order = (theta + north_turn(center, target)).rem_euclid(360.0);
                let apart = (pa - first_order + 180.0).rem_euclid(360.0) - 180.0;
                assert!(
                    apart.abs() < 0.1,
                    "{theta} at {target:?}: {pa} against {first_order}"
                );
                let own = TangentPlane::at(target).unwrap();
                let (sin, cos) = pa.to_radians().sin_cos();
                let top = plane
                    .project(own.deproject([0.6 * sin, 0.6 * cos]))
                    .unwrap();
                let bottom = plane
                    .project(own.deproject([-0.6 * sin, -0.6 * cos]))
                    .unwrap();
                let drawn = (top[0] - bottom[0]).atan2(top[1] - bottom[1]).to_degrees();
                let apart = (drawn - theta + 180.0).rem_euclid(360.0) - 180.0;
                assert!(apart.abs() < 1e-7, "{theta} at {target:?}: drawn {drawn}");
                // Without the turn it draws off by the lean.
                let (sin0, cos0) = theta.to_radians().sin_cos();
                let top0 = plane
                    .project(own.deproject([0.6 * sin0, 0.6 * cos0]))
                    .unwrap();
                let bottom0 = plane
                    .project(own.deproject([-0.6 * sin0, -0.6 * cos0]))
                    .unwrap();
                let drawn0 = (top0[0] - bottom0[0])
                    .atan2(top0[1] - bottom0[1])
                    .to_degrees();
                let off = (drawn0 - theta + 180.0).rem_euclid(360.0) - 180.0;
                let lean = north_turn(center, target);
                if lean.abs() > 0.1 {
                    assert!((off + lean).abs() < 0.05, "{off} against {lean}");
                }
            }
        }
    }

    #[test]
    fn each_planned_panel_is_shot_at_the_framing_angle_turned_by_north_lean() {
        let mut request = request();
        request.center = at(100.0, 60.0);
        request.position_angle_degrees = 20.0;
        request.panel = PanelSize {
            width_degrees: 3.0,
            height_degrees: 2.0,
        };
        request.layout = Layout {
            min_overlap_percent: 0,
            panels: vec![
                PanelPlacement::at("r1c1", 0.0, 0.0),
                PanelPlacement::at("p2", -1.0, 0.0),
                PanelPlacement::at("p3", 1.0, 0.0),
            ],
        };
        let preview = request.preview().unwrap();
        assert!((preview.panels[0].position_angle_degrees - 20.0).abs() < 1e-9);
        for panel in &preview.panels[1..] {
            let lean = north_turn(request.center, panel.footprint.center);
            assert!(lean.abs() > 1.0, "{lean}");
            let exact = camera_angle_at(request.center, panel.footprint.center, 20.0);
            assert!((panel.position_angle_degrees - exact).abs() < 1e-8);
            assert!((panel.position_angle_degrees - (20.0 + lean).rem_euclid(360.0)).abs() < 0.1);
        }
    }

    #[test]
    fn a_turned_panel_is_drawn_checked_and_shot_at_its_own_angle() {
        let mut request = request();
        request.center = at(100.0, 60.0);
        request.position_angle_degrees = 20.0;
        request.panel = PanelSize {
            width_degrees: 3.0,
            height_degrees: 2.0,
        };
        let mut turned = PanelPlacement::at("p2", -0.9, 0.0);
        turned.angle_offset_degrees = 10.0;
        let mut centered = PanelPlacement::at("r1c1", 0.0, 0.0);
        centered.angle_offset_degrees = -15.0;
        request.layout = Layout {
            min_overlap_percent: 10,
            panels: vec![centered, turned],
        };
        let preview = request.preview().unwrap();
        // At the center no lean: the framing's angle plus the panel's turn.
        assert!((preview.panels[0].position_angle_degrees - 5.0).abs() < 1e-9);
        assert_eq!(preview.panels[0].angle_offset_degrees, -15.0);
        // Off the center, the same turn plus north's lean there.
        let p2 = &preview.panels[1];
        let exact = camera_angle_at(request.center, p2.footprint.center, 30.0);
        assert!((p2.position_angle_degrees - exact).abs() < 1e-8);
        let lean = north_turn(request.center, p2.footprint.center);
        assert!((p2.position_angle_degrees - (30.0 + lean)).abs() < 0.1);
        // Its outline in the camera frame is the rectangle turned the same
        // way: its up edge leans toward the camera's left.
        let shapes = request.shapes().unwrap();
        let [top_left, bottom_left, _, _] = shapes[1].corners;
        let up = [top_left[0] - bottom_left[0], top_left[1] - bottom_left[1]];
        let drawn = (-up[0]).atan2(up[1]).to_degrees();
        assert!((drawn - 10.0).abs() < 1e-9, "{drawn}");
        // And on the sky, the footprint's up edge runs at the panel's angle
        // on the plane.
        let plane = TangentPlane::at(request.center).unwrap();
        let [tl, bl, _, _] = p2.footprint.corners.map(|c| plane.project(c).unwrap());
        let sky_up = (tl[0] - bl[0]).atan2(tl[1] - bl[1]).to_degrees();
        assert!((sky_up - 30.0).abs() < 1e-6, "{sky_up}");
        // Overlap is read from the turned outlines: two 3° panels 0.9 apart
        // overlap by about a tenth, less where the turn opens the seam.
        assert!(preview
            .issues
            .iter()
            .all(|issue| issue.kind != LayoutIssueKind::Gap));
        // A turn past a half turn either way is refused.
        let mut wild = request.clone();
        wild.layout.panels[1].angle_offset_degrees = 181.0;
        assert_eq!(wild.preview().unwrap_err(), FramingError::InvalidMosaic);
    }

    fn measured(center: IcrsPosition, angle: f64, width: f64, height: f64) -> MeasuredGeometry {
        MeasuredGeometry {
            center,
            position_angle_degrees: angle,
            size: PanelSize {
                width_degrees: width,
                height_degrees: height,
            },
            source: MeasuredSource::Solved,
            size_source: MeasuredSource::Solved,
            frames: 12,
        }
    }

    /// A locked panel follows its original shots: drawn as shot, kept at
    /// its angle, and the new panels beside it are read against that outline.
    #[test]
    fn a_shot_panel_is_drawn_and_checked_as_shot() {
        let mut request = request();
        request.center = at(38.2, 61.45);
        request.position_angle_degrees = 0.0;
        request.panel = PanelSize {
            width_degrees: 1.8,
            height_degrees: 1.2,
        };
        // Shot with a wider camera, a degree turned and a hair off the plan.
        let mut shot = PanelPlacement::at("r1c1", 0.0, 0.0);
        shot.locked = true;
        shot.measured = Some(measured(at(38.21, 61.452), 1.0, 2.1, 1.4));
        // A new panel to its left (east), sharing a fifth of the wider width.
        let new_x = -(1.05 + 0.9 - 0.2 * 2.1) / 1.8;
        request.layout = Layout {
            min_overlap_percent: 20,
            panels: vec![shot.clone(), PanelPlacement::at("p2", new_x, 0.0)],
        };
        let preview = request.preview().unwrap();
        let drawn = &preview.panels[0];
        assert_eq!(drawn.footprint.center, at(38.21, 61.452));
        assert_eq!(drawn.position_angle_degrees, 1.0);
        assert_eq!(drawn.measured, shot.measured);
        let shapes = request.shapes().unwrap();
        let b = shapes[0].bounds();
        assert!((b[1] - b[0] - 2.1).abs() < 0.06, "{b:?}");
        // The plane's projection and the degree of turn leave the seam a
        // little off a clean 20%: still joined, or at most a thin share.
        let pair = shape::shape_overlap(&shapes[0], &shapes[1]).unwrap();
        assert!((pair.thickness - 0.2).abs() < 0.03, "{pair:?}");
        // Moved half a panel further, it leaves a gap the rule names.
        request.layout.panels[1].x -= 0.5;
        let issues = request.issues().unwrap();
        assert_eq!(issues[0].kind, LayoutIssueKind::Gap, "{issues:?}");
        // Only a locked panel may follow its shots.
        let mut loose = shot;
        loose.locked = false;
        request.layout.panels[0] = loose;
        assert_eq!(request.preview(), Err(FramingError::InvalidMosaic));
    }

    /// A framing at `center` and `angle` with `panel`-sized panels at
    /// `places` (in panel sizes), the first `r1c1`.
    fn framing_at(
        center: IcrsPosition,
        angle: f64,
        panel: (f64, f64),
        places: &[(f64, f64)],
    ) -> FramingRequest {
        FramingRequest {
            center,
            position_angle_degrees: angle,
            panel: PanelSize {
                width_degrees: panel.0,
                height_degrees: panel.1,
            },
            layout: Layout {
                min_overlap_percent: 20,
                panels: places
                    .iter()
                    .enumerate()
                    .map(|(k, &(x, y))| {
                        let id = if k == 0 {
                            "r1c1".to_string()
                        } else {
                            format!("p{}", k + 1)
                        };
                        PanelPlacement::at(id, x, y)
                    })
                    .collect(),
            },
            overlays: vec![],
            view: None,
        }
    }

    fn coverage(framings: &[FramingRequest]) -> Vec<(LayoutIssueKind, Vec<(usize, String)>)> {
        coverage_issues(framings)
            .unwrap()
            .into_iter()
            .map(|issue| {
                (
                    issue.kind,
                    issue
                        .panels
                        .into_iter()
                        .map(|panel| (panel.framing, panel.id))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn one_framing_covers_the_sky_as_its_own_issues_say() {
        let center = request().center;
        let layouts: [&[(f64, f64)]; 4] = [
            &[(0.0, 0.0), (0.9, 0.0)],
            &[(0.0, 0.0), (1.25, 0.0)],
            &[(0.0, 0.0), (3.0, 3.0)],
            &[(0.0, 0.0), (0.8, 0.0), (4.0, 0.0), (4.8, 0.0)],
        ];
        for places in layouts {
            let alone = framing_at(center, 35.0, (2.0, 1.5), places);
            let own = alone.issues().unwrap();
            let read = coverage_issues(std::slice::from_ref(&alone)).unwrap();
            assert_eq!(read.len(), own.len(), "{places:?}");
            for (a, b) in read.iter().zip(&own) {
                assert_eq!(a.kind, b.kind);
                assert_eq!(a.percent, b.percent);
                let ids: Vec<&str> = a.panels.iter().map(|p| p.id.as_str()).collect();
                assert_eq!(ids, b.panels.iter().map(String::as_str).collect::<Vec<_>>());
                assert!(a.panels.iter().all(|p| p.framing == 0));
            }
        }
        assert!(coverage_issues(&[]).unwrap().is_empty());
    }

    #[test]
    fn another_framings_panels_fill_a_rigs_gap() {
        let center = request().center;
        // A rig's own two small panels half a panel apart, either side of
        // the shared framing's one large panel.
        let own = framing_at(center, 0.0, (1.0, 0.75), &[(-0.75, 0.0), (0.75, 0.0)]);
        assert_eq!(
            coverage(std::slice::from_ref(&own)),
            vec![(
                LayoutIssueKind::Gap,
                vec![(0, "r1c1".to_string()), (0, "p2".to_string())]
            )]
        );
        let shared = framing_at(center, 0.0, (2.0, 1.5), &[(0.0, 0.0)]);
        assert!(coverage(&[shared.clone(), own.clone()]).is_empty());
        // Either order reads the same.
        assert!(coverage(&[own, shared]).is_empty());
    }

    #[test]
    fn panels_of_two_framings_join_when_they_touch() {
        let center = request().center;
        let shared = framing_at(center, 0.0, (2.0, 1.5), &[(0.0, 0.0)]);
        // A rig's panel sharing 5% of the larger width: thin within one
        // framing, but across two the sky is covered.
        let beside = |x: f64| framing_at(center, 0.0, (1.0, 0.75), &[(x, 0.0)]);
        assert!(coverage(&[shared.clone(), beside(1.4)]).is_empty());
        let one = framing_at(center, 0.0, (2.0, 1.5), &[(0.0, 0.0), (0.95, 0.0)]);
        assert_eq!(one.issues().unwrap()[0].kind, LayoutIssueKind::ThinOverlap);
        // A tenth of a degree apart, 5% of the larger width: a gap across the two.
        let read = coverage_issues(&[shared.clone(), beside(1.6)]).unwrap();
        assert_eq!(read.len(), 1, "{read:?}");
        assert_eq!(read[0].kind, LayoutIssueKind::Gap);
        assert_eq!(read[0].percent, Some(5.0));
        assert_eq!(
            read[0].panels,
            vec![
                CoveragePanel {
                    framing: 0,
                    id: "r1c1".into()
                },
                CoveragePanel {
                    framing: 1,
                    id: "r1c1".into()
                },
            ]
        );
        // Far apart, each is on its own.
        assert_eq!(
            coverage(&[shared, beside(5.0)]),
            vec![
                (LayoutIssueKind::Isolated, vec![(0, "r1c1".to_string())]),
                (LayoutIssueKind::Isolated, vec![(1, "r1c1".to_string())]),
            ]
        );
    }

    #[test]
    fn groups_of_two_framings_that_never_meet_are_disconnected() {
        let center = request().center;
        let north = IcrsPosition {
            ra_degrees: center.ra_degrees,
            dec_degrees: center.dec_degrees + 5.0,
        };
        let shared = framing_at(center, 0.0, (2.0, 1.5), &[(0.0, 0.0), (0.8, 0.0)]);
        let own = framing_at(north, 20.0, (1.0, 0.75), &[(0.0, 0.0), (0.8, 0.0)]);
        assert_eq!(
            coverage(&[shared, own]),
            vec![(
                LayoutIssueKind::Disconnected,
                vec![(0, "r1c1".to_string()), (1, "r1c1".to_string())]
            )]
        );
    }

    #[test]
    fn groups_apart_across_framings_are_said_beside_other_issues() {
        let center = request().center;
        let north = IcrsPosition {
            ra_degrees: center.ra_degrees,
            dec_degrees: center.dec_degrees + 10.0,
        };
        // The shared pair overlaps thinly, an issue of its own; the rig's
        // pair ten degrees north is joined, but far from everything else.
        let shared = framing_at(center, 0.0, (2.0, 1.5), &[(0.0, 0.0), (0.9, 0.0)]);
        let own = framing_at(north, 0.0, (1.0, 0.75), &[(0.0, 0.0), (0.8, 0.0)]);
        assert_eq!(
            coverage(&[shared.clone(), own]),
            vec![
                (
                    LayoutIssueKind::ThinOverlap,
                    vec![(0, "r1c1".to_string()), (0, "p2".to_string())]
                ),
                (
                    LayoutIssueKind::Disconnected,
                    vec![(0, "r1c1".to_string()), (1, "r1c1".to_string())]
                ),
            ]
        );
        // A lone panel far away is said as such, not again as a group.
        let lone = framing_at(north, 0.0, (1.0, 0.75), &[(0.0, 0.0)]);
        assert_eq!(
            coverage(&[shared.clone(), lone]),
            vec![
                (
                    LayoutIssueKind::ThinOverlap,
                    vec![(0, "r1c1".to_string()), (0, "p2".to_string())]
                ),
                (LayoutIssueKind::Isolated, vec![(1, "r1c1".to_string())]),
            ]
        );
        // One framing keeps the old rule: groups apart only when nothing
        // else is said.
        let alone = framing_at(
            center,
            0.0,
            (2.0, 1.5),
            &[(0.0, 0.0), (0.9, 0.0), (5.0, 0.0), (5.8, 0.0)],
        );
        assert_eq!(
            coverage(std::slice::from_ref(&alone)),
            vec![(
                LayoutIssueKind::ThinOverlap,
                vec![(0, "r1c1".to_string()), (0, "p2".to_string())]
            )]
        );
    }

    #[test]
    fn a_framings_own_seams_are_read_in_its_own_frame() {
        // A rig turned 30° from the shared framing: its thin seam reads as
        // its own issues read it, not along the shared framing's axes.
        let center = request().center;
        let south = IcrsPosition {
            ra_degrees: center.ra_degrees,
            dec_degrees: center.dec_degrees - 6.0,
        };
        let shared = framing_at(south, 0.0, (2.0, 1.5), &[(0.0, 0.0)]);
        let own = framing_at(center, 30.0, (1.0, 0.75), &[(0.0, 0.0), (0.9, 0.0)]);
        let alone = own.issues().unwrap();
        assert_eq!(alone[0].kind, LayoutIssueKind::ThinOverlap);
        let read = coverage_issues(&[shared, own]).unwrap();
        let thin = read
            .iter()
            .find(|issue| issue.kind == LayoutIssueKind::ThinOverlap)
            .unwrap();
        assert_eq!(thin.percent, alone[0].percent);
        assert!(thin.panels.iter().all(|panel| panel.framing == 1));
        assert!(read
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::Isolated && issue.panels[0].framing == 0));
    }
}
