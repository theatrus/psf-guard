//! Footprint and mosaic geometry for the framing view. Pure arithmetic on a
//! tangent plane around the project's center; the browser only draws what
//! this returns. A footprint says where a rig would point, not where it did.

use crate::visibility::IcrsPosition;
use serde::{Deserialize, Serialize};

pub mod layout;
pub use layout::{Layout, LayoutIssue, LayoutIssueKind, PanelPlacement};

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
    #[serde(flatten)]
    pub footprint: Footprint,
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
        let extent = self.extent();
        if extent.width_degrees > MAX_EXTENT_DEGREES || extent.height_degrees > MAX_EXTENT_DEGREES {
            return Err(FramingError::TooLarge);
        }
        // Every panel within the reach where the flat layout stays honest.
        if self.layout.panels.iter().any(|p| {
            (p.x * self.panel.width_degrees).abs() > MAX_EXTENT_DEGREES
                || (p.y * self.panel.height_degrees).abs() > MAX_EXTENT_DEGREES
        }) {
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

    /// The whole mosaic along the camera axes, edge to edge.
    pub fn extent(&self) -> PanelSize {
        let (across, up) = self.layout.span();
        PanelSize {
            width_degrees: self.panel.width_degrees * across,
            height_degrees: self.panel.height_degrees * up,
        }
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
        let footprint = |id: String, center: Offset, size: PanelSize, angle: f64| {
            let corners = rectangle(center, size, angle).map(|c| plane.deproject(c));
            let view_corners = if view.is_some() {
                let mut mapped = [[0.0; 2]; 4];
                for (slot, corner) in mapped.iter_mut().zip(corners) {
                    *slot = to_view(corner)?;
                }
                Some(mapped)
            } else {
                None
            };
            Ok::<_, FramingError>(Footprint {
                id,
                center: plane.deproject(center),
                corners,
                view_corners,
            })
        };
        let (sin, cos) = self.position_angle_degrees.to_radians().sin_cos();
        let up = [sin, cos];
        let right = [-cos, sin];
        let cells = self.layout.cells();
        let mut panels = Vec::with_capacity(self.layout.panels.len());
        for (placement, (row, column)) in self.layout.panels.iter().zip(cells) {
            // Offsets along the camera's axes, in degrees on the plane.
            let dx = placement.x * self.panel.width_degrees;
            let dy = placement.y * self.panel.height_degrees;
            let center = [dx * right[0] + dy * up[0], dx * right[1] + dy * up[1]];
            panels.push(Panel {
                row,
                column,
                locked: placement.locked,
                footprint: footprint(
                    placement.id.clone(),
                    center,
                    self.panel,
                    self.position_angle_degrees,
                )?,
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
            issues: self.layout.issues(),
        })
    }
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
        request.layout.panels.push(PanelPlacement {
            id: "p2".into(),
            x: -0.8,
            y: 0.0,
            locked: false,
        });
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
                    PanelPlacement {
                        id: "r1c1".into(),
                        x: 0.0,
                        y: 0.0,
                        locked: false,
                    },
                    PanelPlacement {
                        id: "p2".into(),
                        x: 0.8,
                        y: 0.0,
                        locked: false,
                    },
                    PanelPlacement {
                        id: "p3".into(),
                        x: 0.0,
                        y: 0.8,
                        locked: false,
                    },
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
}
