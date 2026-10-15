import type { DirectorFootprint, DirectorFramingDraft, DirectorFramingPreview, DirectorFramingRequest, DirectorLayout, DirectorMeasuredGeometry, DirectorMosaic, DirectorOffset, DirectorPanelSize, DirectorRigProfileSummary, DirectorSkyPosition, DirectorSurvey, DirectorRigFraming} from '../../api/directorTypes';
import type { SkyPreview } from '../../api/types';
import { formatDecShort, formatRaShort, galacticBandQuads, tanPixelToSky } from '../../utils/skyProjection';
import { BRIGHT_STARS, CONSTELLATION_LINES, CONSTELLATION_NAMES } from '../../data/skyBackdrop';
import { gridLayout, layoutStructureProblem, singleLayout, turnOf, type BoxOf } from './mosaicLayout';
import { boundsOf, readShapes, readTagged, shapeCenter, turnedShape, type Bounds, type PanelShape, type Point, type ShapeReading, type TaggedIssue } from './panelShape';

/** The stage's logical pixels; CSS scales the drawing to the element. The
 *  longer side is always 1024, so a 4:3 stage is 1024 × 768. */
export interface Stage { width: number; height: number }
export const STAGE_WIDTH = 1024;
export const STAGE_HEIGHT = 768;
export const DEFAULT_STAGE: Stage = { width: STAGE_WIDTH, height: STAGE_HEIGHT };
/** The stage for an element of this width-to-height ratio. */
export function stageFor(aspect: number): Stage {
  const a = Number.isFinite(aspect) && aspect > 0 ? aspect : 4 / 3;
  return a >= 1 ? { width: STAGE_WIDTH, height: Math.round(STAGE_WIDTH / a) } : { width: Math.round(STAGE_WIDTH * a), height: STAGE_WIDTH };
}
export const MIN_VIEW_FOV = 0.05;
/** Width of view in stage degrees. The stage is stereographic, so 150 stage
 *  degrees is a hemisphere and a bit less across on the sky. */
export const MAX_VIEW_FOV = 150;
/** The widest survey image the server renders: a hemisphere and a bit,
 *  so the sky has a picture behind it at every zoom. */
export const TILE_MAX_FOV = 180;

/** A catalog target the draft starts from when no draft exists yet. */
export interface FramingSeed { name: string; center: DirectorSkyPosition; position_angle_degrees: number }

export interface FramingState {
  targetName: string;
  center: DirectorSkyPosition;
  positionAngle: number;
  /** The shared framing's panels, placed one by one around the center. */
  layout: DirectorLayout;
  panelRigId: string | null;
  panel: DirectorPanelSize | null;
  shownRigIds: string[];
  surveyId: string;
  viewCenter: DirectorSkyPosition;
  viewFov: number;
  /** Rigs framed on their own; every other rig shoots the shared framing. */
  rigFramings: DirectorRigFraming[];
}

/** A draft's panels; one written before panels were placed one by one
 *  carries a grid instead, which makes the same panels in the same places. */
export function layoutOf(source: { layout?: DirectorLayout | null; mosaic?: DirectorMosaic | null }): DirectorLayout {
  return source.layout ?? (source.mosaic ? gridLayout(source.mosaic) : singleLayout());
}

export function stateFromDraft(draft: DirectorFramingDraft): FramingState {
  // Angles are read through the same rounding every edit uses, so one saved
  // with float drift before is not an edit when a turn comes back to it.
  return {
    targetName: draft.target_name, center: draft.center, positionAngle: normaliseAngle(draft.position_angle_degrees),
    layout: layoutOf(draft), panelRigId: draft.panel_rig_id, panel: draft.panel, shownRigIds: draft.shown_rig_ids,
    surveyId: draft.survey_id, viewCenter: draft.center, viewFov: clampFov(draft.view_fov_degrees),
    rigFramings: (draft.rig_framings ?? []).map(own => {
      const read = { ...own, layout: layoutOf(own) };
      return read.position_angle_degrees === null ? read : { ...read, position_angle_degrees: normaliseAngle(read.position_angle_degrees) };
    }),
  };
}

/** An angle, or a right ascension, in [0, 360): wrapped, with the last
 *  digits of float drift rounded off so a quarter turn and back is the
 *  same number, and a value that rounds up to 360 read as 0. The core
 *  refuses 360 itself. Nine places keep every digit anyone types. */
export function normaliseAngle(degrees: number): number {
  if (!Number.isFinite(degrees)) return 0;
  const rounded = Math.round((((degrees % 360) + 360) % 360) * 1e9) / 1e9;
  return rounded >= 360 ? 0 : rounded;
}

/** A right ascension rounded for a request: `toFixed` turns 359.999996
 *  into 360, which the sky and feasibility endpoints refuse. */
export function roundRa(raDegrees: number, places: number): number {
  const rounded = Number(normaliseAngle(raDegrees).toFixed(places));
  return rounded >= 360 ? 0 : rounded;
}

/** The core's limits on a framing (`crates/director-core/src/framing.rs`):
 *  a panel side and a whole mosaic are at most 30° across, a panel side
 *  at least 0.01°. */
export const MAX_EXTENT_DEGREES = 30;
export const MIN_PANEL_DEGREES = 0.01;

/** The mosaic's extent along the camera axes, as the core computes it:
 *  planned panels at their places, shot ones as shot. */
export function mosaicExtent(center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize, layout: DirectorLayout): DirectorPanelSize {
  return extentOf(shapesOf(center, positionAngle, panel, layout));
}

function extentOf(shapes: PanelShape[]): DirectorPanelSize {
  if (shapes.length === 0) return { width_degrees: 0, height_degrees: 0 };
  const b = boundsOf(shapes.flatMap(shape => shape.corners));
  return { width_degrees: b[1] - b[0], height_degrees: b[3] - b[2] };
}

/** Something the server would refuse to save, named by the field it is in. */
export interface FramingProblem {
  /** `center`, `angle`, `panel` or `mosaic` for the shared framing; the
   *  same after `rig:<id>:` for a rig framed on its own. */
  field: string;
  message: string;
}

function layoutProblems(prefix: string, who: string, center: DirectorSkyPosition, angle: number, panel: DirectorPanelSize | null, layout: DirectorLayout): FramingProblem[] {
  const problems: FramingProblem[] = [];
  const say = (field: string, message: string) => problems.push({ field: `${prefix}${field}`, message: who ? `${who}${message}` : message[0].toUpperCase() + message.slice(1) });
  const within = (value: number, low: number, high: number) => Number.isFinite(value) && value >= low && value <= high;
  if (!within(center.ra_degrees, 0, 360) || center.ra_degrees >= 360 || !within(center.dec_degrees, -90, 90)) say('center', 'RA 0 to 360°, Dec −90 to 90°');
  if (!within(angle, 0, 360) || angle >= 360) say('angle', 'camera angle 0 to 360°');
  const structure = layoutStructureProblem(layout);
  if (structure) say('mosaic', structure);
  if (panel) {
    if (![panel.width_degrees, panel.height_degrees].every(side => within(side, MIN_PANEL_DEGREES, MAX_EXTENT_DEGREES))) say('panel', `panel sides ${MIN_PANEL_DEGREES}° to ${MAX_EXTENT_DEGREES}°`);
    else if (!structure) {
      // Every panel within the reach the flat layout stays honest in, a
      // shot one by where it was shot.
      const far = layout.panels.some(p => {
        const at = p.measured ? offsetFrom(center, p.measured.center) : [p.x * panel.width_degrees, p.y * panel.height_degrees];
        return !at || Math.abs(at[0]) > MAX_EXTENT_DEGREES || Math.abs(at[1]) > MAX_EXTENT_DEGREES;
      });
      const extent = far ? null : mosaicExtent(center, angle, panel, layout);
      if (!extent) say('mosaic', `a panel more than ${MAX_EXTENT_DEGREES}° from the center`);
      else if (extent.width_degrees > MAX_EXTENT_DEGREES || extent.height_degrees > MAX_EXTENT_DEGREES) say('mosaic', `mosaic ${formatDegrees(extent.width_degrees)} × ${formatDegrees(extent.height_degrees)}, over ${MAX_EXTENT_DEGREES}°`);
    }
  }
  return problems;
}

/** What the server would refuse in this framing, checked here first so the
 *  field can say so before a save, not after a bare 400. A separate framing
 *  is sized from its rig's field when no size is typed, as activation sizes it. */
export function framingProblems(state: FramingState, rigs: DirectorRigProfileSummary[], rigName: (id: string) => string = () => 'A rig'): FramingProblem[] {
  const problems = layoutProblems('', '', state.center, state.positionAngle, state.panel, state.layout);
  for (const own of state.rigFramings) {
    problems.push(...layoutProblems(`rig:${own.rig_id}:`, `${rigName(own.rig_id)}: `, own.center ?? state.center, own.position_angle_degrees ?? state.positionAngle, own.panel ?? panelForRig(rigs, own.rig_id), own.layout));
  }
  return problems;
}

export function stateFromSeed(seed: FramingSeed, defaultSurvey: string): FramingState {
  return {
    targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees,
    layout: singleLayout(20), panelRigId: null, panel: null, shownRigIds: [],
    surveyId: defaultSurvey, viewCenter: seed.center, viewFov: 4, rigFramings: [],
  };
}

export function draftFromState(state: FramingState, projectId: string, revision: number): DirectorFramingDraft {
  return {
    project_id: projectId, revision, target_name: state.targetName.trim(), center: state.center,
    position_angle_degrees: state.positionAngle, layout: state.layout, panel_rig_id: state.panelRigId,
    panel: state.panel, shown_rig_ids: state.shownRigIds, survey_id: state.surveyId,
    // The view's width is the view's own: it is kept with the draft but
    // never part of the layout, and always within what the stage shows.
    view_fov_degrees: clampFov(state.viewFov), updated_at_ms: 0, rig_framings: state.rigFramings,
  };
}

/** What one rig shoots, the way the server lays it out: its own framing when
 *  it has one (its field standing in for a size not set by hand), else the
 *  shared framing. Null when no panel size is known. */
export function rigLayout(state: FramingState, rigId: string, rigs: DirectorRigProfileSummary[]): { center: DirectorSkyPosition; positionAngle: number; panel: DirectorPanelSize; layout: DirectorLayout; own: boolean } | null {
  const own = state.rigFramings.find(entry => entry.rig_id === rigId);
  if (own) {
    const panel = own.panel ?? panelForRig(rigs, rigId);
    return panel ? { center: own.center ?? state.center, positionAngle: own.position_angle_degrees ?? state.positionAngle, panel, layout: own.layout, own: true } : null;
  }
  return state.panel ? { center: state.center, positionAngle: state.positionAngle, panel: state.panel, layout: state.layout, own: false } : null;
}

/** The geometry of every rig framed on its own, for the stage. */
export function rigGeometries(state: FramingState, rigs: DirectorRigProfileSummary[]): Array<{ rigId: string; geometry: DirectorFramingPreview; layout: NonNullable<ReturnType<typeof rigLayout>> }> {
  return state.rigFramings.flatMap(own => {
    const layout = rigLayout(state, own.rig_id, rigs);
    if (!layout) return [];
    return [{ rigId: own.rig_id, layout, geometry: framingGeometry({ center: layout.center, position_angle_degrees: layout.positionAngle, panel: layout.panel, layout: layout.layout, overlays: [], view: { center: state.viewCenter, rotation_degrees: 0 } }) }];
  });
}

/** The panel a rig would shoot: its field, or nothing until its profile has optics. */
export function panelForRig(rigs: DirectorRigProfileSummary[], rigId: string | null): DirectorPanelSize | null {
  const rig = rigs.find(entry => entry.rig.id === rigId);
  if (!rig?.field_of_view) return null;
  return { width_degrees: rig.field_of_view.width_degrees, height_degrees: rig.field_of_view.height_degrees };
}

export function previewRequest(state: FramingState, rigs: DirectorRigProfileSummary[]): DirectorFramingRequest | null {
  if (!state.panel) return null;
  return {
    center: state.center, position_angle_degrees: state.positionAngle, panel: state.panel, layout: state.layout,
    // A rig framed on its own is drawn as its own grid, not as an overlay.
    overlays: state.shownRigIds.filter(id => !state.rigFramings.some(own => own.rig_id === id)).flatMap(id => {
      const size = panelForRig(rigs, id);
      return size ? [{ id, size, position_angle_degrees: state.positionAngle }] : [];
    }),
    view: { center: state.viewCenter, rotation_degrees: 0 },
  };
}

/** A rig's field outlined where a locked shot panel holds the plan. */
export interface LockedOutline { rigId: string; corners: DirectorFootprint['corners'] }

/** The rigs whose field a locked shot panel hides. A shot panel is drawn
 *  where and how its frames were taken, whatever rig frames the plan, so
 *  turning a rig on or picking it changes nothing in sight. While the shared
 *  framing has a locked shot panel, each rig in `onRigs` that shoots the
 *  shared framing, knows its field and has no frames on a locked shot panel
 *  (`shotBy`) gets its field outlined at that panel's shot center and angle,
 *  where activation keeps the target: the panel at the framing's center,
 *  else the first locked shot one. */
export function lockedOutlines(state: FramingState, rigs: DirectorRigProfileSummary[], onRigs: string[], shotBy: Set<string>): LockedOutline[] {
  const shot = state.layout.panels.filter(panel => panel.locked && panel.measured);
  if (shot.length === 0) return [];
  const anchor = (shot.find(panel => Math.abs(panel.x) < 1e-9 && Math.abs(panel.y) < 1e-9) ?? shot[0]).measured!;
  return onRigs
    .filter(id => !shotBy.has(id) && !state.rigFramings.some(own => own.rig_id === id))
    .flatMap(id => {
      const field = panelForRig(rigs, id);
      return field ? [{ rigId: id, corners: measuredCorners({ ...anchor, size: field }) }] : [];
    });
}

/** Degrees per stage pixel at the center of the view. */
export function pixelScale(viewFov: number, stage: Stage = DEFAULT_STAGE): number {
  return viewFov / stage.width;
}

/** North up, east left: a positive east offset moves left on the stage. */
export function toStage(offset: DirectorOffset, viewFov: number, stage: Stage = DEFAULT_STAGE): [number, number] {
  const scale = pixelScale(viewFov, stage);
  return [stage.width / 2 - offset[0] / scale, stage.height / 2 - offset[1] / scale];
}

export function polygonPoints(corners: DirectorOffset[], viewFov: number, stage: Stage = DEFAULT_STAGE): string {
  return corners.map(corner => toStage(corner, viewFov, stage).map(v => v.toFixed(1)).join(',')).join(' ');
}

/** The stage's own projection: stereographic about the view center, in
 *  degrees east and north as the tangent plane counts them at the center.
 *  It matches the tangent plane to second order, so at framing widths the
 *  two agree to a pixel, and it stays bounded out to a hemisphere, so one
 *  projection serves the chart at every zoom. The server's survey images
 *  and offline maps are rendered in it too. `null` at the antipode only. */
export function stageProject(view: DirectorSkyPosition, position: DirectorSkyPosition): DirectorOffset | null {
  const rad = Math.PI / 180;
  const dec0 = view.dec_degrees * rad;
  const dec = position.dec_degrees * rad;
  const dra = (position.ra_degrees - view.ra_degrees) * rad;
  const cosC = Math.sin(dec0) * Math.sin(dec) + Math.cos(dec0) * Math.cos(dec) * Math.cos(dra);
  if (!(cosC > -1 + 1e-9)) return null;
  const k = 2 / (1 + cosC);
  const xi = k * Math.cos(dec) * Math.sin(dra);
  const eta = k * (Math.cos(dec0) * Math.sin(dec) - Math.sin(dec0) * Math.cos(dec) * Math.cos(dra));
  return [xi / rad, eta / rad];
}

/** The sky position at a stage offset from the view center: the exact inverse of `stageProject`. */
export function stageDeproject(view: DirectorSkyPosition, offset: DirectorOffset): DirectorSkyPosition {
  const rad = Math.PI / 180;
  const xi = offset[0] * rad;
  const eta = offset[1] * rad;
  const rho = Math.hypot(xi, eta);
  if (rho < 1e-12) return { ...view };
  const c = 2 * Math.atan(rho / 2);
  const sinC = Math.sin(c);
  const cosC = Math.cos(c);
  const dec0 = view.dec_degrees * rad;
  const dec = Math.asin(Math.max(-1, Math.min(1, cosC * Math.sin(dec0) + (eta * sinC * Math.cos(dec0)) / rho)));
  const ra = view.ra_degrees * rad + Math.atan2(xi * sinC, rho * Math.cos(dec0) * cosC - eta * Math.sin(dec0) * sinC);
  return { ra_degrees: (((ra / rad) % 360) + 360) % 360, dec_degrees: dec / rad };
}

/** What the stage looks at: a stereographic plane about the view center,
 *  as N.I.N.A.'s framing assistant and the Sky view have it, so a drag
 *  turns the globe under the pointer and a footprint away from the center
 *  leans with its local north. `offset` lets a window sit off the plane's
 *  center; the framing view keeps it at zero. */
export interface StageView {
  anchor: DirectorSkyPosition;
  offset: DirectorOffset;
  /** How far the sky is turned on the stage, east of north: 0 keeps north
   *  up; the camera angle keeps the rectangle upright and turns the sky,
   *  as N.I.N.A.'s "rotate sky" does. */
  rotation: number;
}

/** The view whose window is centered on `viewCenter` over the plane at `anchor`. */
export function viewAt(anchor: DirectorSkyPosition, viewCenter: DirectorSkyPosition, rotation = 0): StageView {
  return { anchor, offset: stageProject(anchor, viewCenter) ?? [0, 0], rotation };
}

function turned(offset: DirectorOffset, degrees: number): DirectorOffset {
  if (degrees === 0) return offset;
  const rad = (degrees * Math.PI) / 180;
  return [offset[0] * Math.cos(rad) - offset[1] * Math.sin(rad), offset[0] * Math.sin(rad) + offset[1] * Math.cos(rad)];
}

/** A sky position in window coordinates: degrees right and up of the window's
 *  center, east to the left while the sky is not turned. */
export function projectOn(view: StageView, position: DirectorSkyPosition): DirectorOffset | null {
  const at = stageProject(view.anchor, position);
  return at ? turned([at[0] - view.offset[0], at[1] - view.offset[1]], view.rotation) : null;
}

/** The sky position at window coordinates: the inverse of `projectOn`. */
export function deprojectOn(view: StageView, offset: DirectorOffset): DirectorSkyPosition {
  const [x, y] = turned(offset, -view.rotation);
  return stageDeproject(view.anchor, [x + view.offset[0], y + view.offset[1]]);
}

/** Where north and east point on the stage, as unit vectors in stage
 *  pixels (x right, y down), for the compass. */
export function compassDirections(rotation: number): { north: [number, number]; east: [number, number] } {
  const rad = (rotation * Math.PI) / 180;
  return { north: [Math.sin(rad), -Math.cos(rad)], east: [-Math.cos(rad), -Math.sin(rad)] };
}

/** How much sky the stage really spans across: less than the stage degrees
 *  once the view is wide, since the projection stretches toward the rim. */
export function trueWidth(viewFov: number): number {
  const rho = (viewFov / 2) * (Math.PI / 180);
  return (4 * Math.atan(rho / 2) * 180) / Math.PI;
}

/** Where the mouse points on the sky, from stage pixels. */
export function skyAtStage(view: StageView, viewFov: number, x: number, y: number, stage: Stage = DEFAULT_STAGE): DirectorSkyPosition {
  return deprojectOn(view, fromStage(x, y, viewFov, stage));
}

export function moveBy(position: DirectorSkyPosition, xiDegrees: number, etaDegrees: number): DirectorSkyPosition {
  const dec = Math.max(-90, Math.min(90, position.dec_degrees + etaDegrees));
  const cos = Math.cos((position.dec_degrees * Math.PI) / 180);
  const ra = cos < 1e-6 ? position.ra_degrees : position.ra_degrees + xiDegrees / cos;
  return { ra_degrees: ((ra % 360) + 360) % 360, dec_degrees: dec };
}

/** Where a sky position falls on the view's tangent plane, in degrees east
 *  and north of the view center; the same gnomonic projection the server's
 *  framing preview uses. `null` beyond the plane's horizon. */
export function offsetFrom(view: DirectorSkyPosition, position: DirectorSkyPosition): DirectorOffset | null {
  const rad = Math.PI / 180;
  const dec0 = view.dec_degrees * rad;
  const dec = position.dec_degrees * rad;
  const dra = (position.ra_degrees - view.ra_degrees) * rad;
  const cosC = Math.sin(dec0) * Math.sin(dec) + Math.cos(dec0) * Math.cos(dec) * Math.cos(dra);
  if (!(cosC > 1e-9)) return null;
  const xi = (Math.cos(dec) * Math.sin(dra)) / cosC;
  const eta = (Math.cos(dec0) * Math.sin(dec) - Math.sin(dec0) * Math.cos(dec) * Math.cos(dra)) / cosC;
  return [xi / rad, eta / rad];
}

/** SVG matrix that lays a solved stack preview on the stage: its pixel
 *  corners go through its TAN solution to the sky, then onto the stage.
 *  Fitted to three corners, which is exact for the affine a preview needs at
 *  framing scales. `null` when any corner leaves the view. */
export function stackMatrix(preview: SkyPreview, view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): string | null {
  if (!preview.wcs || preview.width <= 0 || preview.height <= 0) return null;
  const { width, height, wcs } = preview;
  return affineFrom(width, height, (x, y) => {
    const [ra, dec] = tanPixelToSky(wcs, x, y);
    return { ra_degrees: ra, dec_degrees: dec };
  }, view, viewFov, stage);
}

/** The SVG matrix that lays a `width` × `height` picture on the stage,
 *  fitted through three of its corners taken to the sky by `skyAt`. */
function affineFrom(width: number, height: number, skyAt: (x: number, y: number) => DirectorSkyPosition, view: StageView, viewFov: number, stage: Stage): string | null {
  const points = ([[0, 0], [width, 0], [0, height]] as const).map(([x, y]) => {
    const offset = projectOn(view, skyAt(x, y));
    return offset ? toStage(offset, viewFov, stage) : null;
  });
  if (points.some(point => point === null)) return null;
  const [p0, p1, p2] = points as [number, number][];
  const a = (p1[0] - p0[0]) / width;
  const b = (p1[1] - p0[1]) / width;
  const c = (p2[0] - p0[0]) / height;
  const d = (p2[1] - p0[1]) / height;
  if (![a, b, c, d, p0[0], p0[1]].every(Number.isFinite)) return null;
  return `matrix(${a} ${b} ${c} ${d} ${p0[0]} ${p0[1]})`;
}

/** The sky position at a tangent-plane offset from `center`, in degrees east
 *  and north: the exact inverse of `offsetFrom`, as the server's core does it. */
export function deprojectFrom(center: DirectorSkyPosition, offset: DirectorOffset): DirectorSkyPosition {
  const rad = Math.PI / 180;
  const xi = offset[0] * rad;
  const eta = offset[1] * rad;
  const dec0 = center.dec_degrees * rad;
  const denominator = Math.cos(dec0) - eta * Math.sin(dec0);
  const ra = center.ra_degrees * rad + Math.atan2(xi, denominator);
  const dec = Math.asin(Math.max(-1, Math.min(1, (Math.sin(dec0) + eta * Math.cos(dec0)) / Math.sqrt(1 + xi * xi + eta * eta))));
  return { ra_degrees: (((ra / rad) % 360) + 360) % 360, dec_degrees: dec / rad };
}

/** Rectangle corners on the plane, counter-clockwise from the top left, with
 *  "up" at `angleDegrees` east of north. Mirrors the core's `rectangle`. */
function rectangle(center: DirectorOffset, size: DirectorPanelSize, angleDegrees: number): [DirectorOffset, DirectorOffset, DirectorOffset, DirectorOffset] {
  const rad = (angleDegrees * Math.PI) / 180;
  const up = [Math.sin(rad), Math.cos(rad)];
  const right = [-Math.cos(rad), Math.sin(rad)];
  const hw = size.width_degrees / 2;
  const hh = size.height_degrees / 2;
  const corner = (sx: number, sy: number): DirectorOffset => [center[0] + sx * hw * right[0] + sy * hh * up[0], center[1] + sx * hw * right[1] + sy * hh * up[1]];
  return [corner(-1, 1), corner(-1, -1), corner(1, -1), corner(1, 1)];
}

/** Unit vectors at a sky position: toward it, east and north. */
function basis(position: DirectorSkyPosition): [number[], number[], number[]] {
  const rad = Math.PI / 180;
  const sa = Math.sin(position.ra_degrees * rad); const ca = Math.cos(position.ra_degrees * rad);
  const sd = Math.sin(position.dec_degrees * rad); const cd = Math.cos(position.dec_degrees * rad);
  return [[cd * ca, cd * sa, sd], [-sa, ca, 0], [-sd * ca, -sd * sa, cd]];
}
const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/** The position angle a camera at `at` takes so its up draws at `theta`
 *  (east of north) on the tangent plane about `center`: `theta` plus north's
 *  lean there, about `Δα · sin δ`. The core's `camera_angle_at`, exactly. */
export function cameraAngleAt(center: DirectorSkyPosition, at: DirectorSkyPosition, theta: number): number {
  const [z, x, y] = basis(center);
  const [p, east, north] = basis(at);
  const px = dot(p, x); const py = dot(p, y); const pz = dot(p, z);
  if (pz <= 1e-9) return normaliseAngle(theta);
  const rad = (theta * Math.PI) / 180;
  const dx = Math.sin(rad); const dy = Math.cos(rad);
  const w = [pz * dy, -pz * dx, py * dx - px * dy];
  let t = [py * w[2] - pz * w[1], pz * w[0] - px * w[2], px * w[1] - py * w[0]];
  const drawn = [t[0] * pz - px * t[2], t[1] * pz - py * t[2]];
  if (drawn[0] * dx + drawn[1] * dy < 0) t = t.map(v => -v);
  const tangent = [0, 1, 2].map(i => t[0] * x[i] + t[1] * y[i] + t[2] * z[i]);
  return normaliseAngle((Math.atan2(dot(tangent, east), dot(tangent, north)) * 180) / Math.PI);
}

/** North's lean at `at` on the plane about `center`, in (-180, 180]. */
export function northTurn(center: DirectorSkyPosition, at: DirectorSkyPosition): number {
  const angle = cameraAngleAt(center, at, 0);
  return ((((angle + 180) % 360) + 360) % 360) - 180;
}

/** A plane offset (east, north) in the camera frame (right, up), degrees. */
export function planeToCameraDegrees(offset: DirectorOffset, positionAngle: number): Point {
  const rad = (positionAngle * Math.PI) / 180;
  return [-offset[0] * Math.cos(rad) + offset[1] * Math.sin(rad), offset[0] * Math.sin(rad) + offset[1] * Math.cos(rad)];
}
/** A camera-frame place (right, up) as a plane offset: the same reflection. */
export const cameraDegreesToPlane = (place: Point, positionAngle: number): DirectorOffset => planeToCameraDegrees(place, positionAngle);

/** A shot panel's corners on the sky: the rectangle its camera covered on
 *  the plane about its own center, at the angle it was shot. */
export function measuredCorners(measured: DirectorMeasuredGeometry): DirectorFootprint['corners'] {
  return rectangle([0, 0], measured.size, measured.position_angle_degrees).map(corner => deprojectFrom(measured.center, corner)) as DirectorFootprint['corners'];
}

/** Every panel's outline in the camera frame, in degrees: planned panels as
 *  rectangles at their places and turns, shot ones as shot, projected onto
 *  the framing's plane. The core's `FramingRequest::shapes`. */
export function framingShapes(center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize, layout: DirectorLayout): PanelShape[] {
  return layout.panels.map(placement => {
    if (placement.measured) {
      const corners = measuredCorners(placement.measured).map(corner => {
        const offset = offsetFrom(center, corner);
        return offset ? planeToCameraDegrees(offset, positionAngle) : [NaN, NaN] as Point;
      });
      return { id: placement.id, corners: corners as PanelShape['corners'] };
    }
    return turnedShape(placement.id, [placement.x * panel.width_degrees, placement.y * panel.height_degrees], panel.width_degrees, panel.height_degrees, turnOf(placement));
  });
}

/** Each layout's outlines and the rule's reading of them, kept per layout
 *  object for the center, angle and panel size they were read at. A layout
 *  is replaced, never changed in place, so a pan, a zoom or an edit
 *  elsewhere reads it again for free; only a panel edit clips every pair
 *  again. */
const readings = new WeakMap<DirectorLayout, { key: string; shapes: PanelShape[]; reading: ShapeReading }>();
const readingKey = (center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize) => `${center.ra_degrees},${center.dec_degrees},${positionAngle},${panel.width_degrees},${panel.height_degrees}`;
export function readFraming(center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize, layout: DirectorLayout): { shapes: PanelShape[]; reading: ShapeReading } {
  const key = readingKey(center, positionAngle, panel);
  const known = readings.get(layout);
  if (known && known.key === key) return known;
  const shapes = framingShapes(center, positionAngle, panel, layout);
  const entry = { key, shapes, reading: readShapes(shapes, layout.min_overlap_percent) };
  readings.set(layout, entry);
  return entry;
}

/** The outlines alone: read already, or drawn without the rule's reading. */
function shapesOf(center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize, layout: DirectorLayout): PanelShape[] {
  const known = readings.get(layout);
  return known && known.key === readingKey(center, positionAngle, panel) ? known.shapes : framingShapes(center, positionAngle, panel, layout);
}

/** One of several framings of one object: the shared framing, or a rig's own. */
export interface CoverageFraming { center: DirectorSkyPosition; positionAngle: number; panel: DirectorPanelSize; layout: DirectorLayout }
/** A panel among several framings: the framing's place in the list read, and the panel's id. */
export interface CoveragePanel { framing: number; id: string }
export interface CoverageIssue { kind: TaggedIssue['kind']; panels: CoveragePanel[]; percent: number | null }

/** Where several framings of one object fall short as one cover of the sky:
 *  the core's `coverage_issues`. Two panels of different framings need the
 *  larger of the two minimum overlaps, counted on the smaller panel, unless
 *  the smaller one is covered beyond their seam by panels it joins. A gap, a
 *  lone panel and groups apart are read over every panel, so a rig's own
 *  panels need not touch where another framing fills the sky between them.
 *  `groups[f][k]` is the group of framing `f`'s panel `k`, as a panel of the
 *  list. */
export function readCoverage(framings: CoverageFraming[]): CoverageReading {
  if (framings.length === 0) return { issues: [], groups: [] };
  // Kept per first layout, for the framings read with it: a pan, a zoom or
  // an edit elsewhere reads it again for free.
  const known = coverReadings.get(framings[0].layout) ?? [];
  const hit = known.find(entry => entry.framings.length === framings.length && entry.framings.every((f, k) => sameCoverageFraming(f, framings[k])));
  if (hit) return hit.reading;
  const reading = readCoverageAfresh(framings);
  coverReadings.set(framings[0].layout, [{ framings, reading }, ...known].slice(0, 4));
  return reading;
}
export interface CoverageReading { issues: CoverageIssue[]; groups: CoveragePanel[][] }
const coverReadings = new WeakMap<DirectorLayout, Array<{ framings: CoverageFraming[]; reading: CoverageReading }>>();
const sameCoverageFraming = (a: CoverageFraming, b: CoverageFraming) => a.layout === b.layout && a.positionAngle === b.positionAngle
  && a.center.ra_degrees === b.center.ra_degrees && a.center.dec_degrees === b.center.dec_degrees
  && a.panel.width_degrees === b.panel.width_degrees && a.panel.height_degrees === b.panel.height_degrees;

function readCoverageAfresh(framings: CoverageFraming[]): CoverageReading {
  const own = framings.map(f => shapesOf(f.center, f.positionAngle, f.panel, f.layout));
  const sky = framings.map((f, g) => own[g].map(shape => shape.corners.map(corner => deprojectFrom(f.center, cameraDegreesToPlane(corner, f.positionAngle)))));
  // Every panel in each framing's camera frame: its own panels as they are,
  // the others' taken there through the sky.
  const frames = framings.map((f, index) => own.flatMap((shapes, g) => g === index ? shapes : shapes.map((shape, k) => ({
    id: shape.id,
    corners: sky[g][k].map(corner => { const offset = offsetFrom(f.center, corner); return offset ? planeToCameraDegrees(offset, f.positionAngle) : [NaN, NaN] as Point; }) as PanelShape['corners'],
  }))));
  const framing = own.flatMap((shapes, f) => shapes.map(() => f));
  const ids = own.flatMap(shapes => shapes.map(shape => shape.id));
  const panelAt = (k: number): CoveragePanel => ({ framing: framing[k], id: ids[k] });
  const read = readTagged(frames, framing, framings.map(f => f.layout.min_overlap_percent));
  const groups = own.map(() => [] as CoveragePanel[]);
  read.groups.forEach((group, k) => groups[framing[k]].push(panelAt(group)));
  return { issues: read.issues.map(issue => ({ ...issue, panels: issue.panels.map(panelAt) })), groups };
}

/** The cover as one framing's tools read it, with that framing's `layout`
 *  in place of its own: its panels by id, the others' as `label id`, and
 *  only the issues that touch its panels, or say the panels fall apart.
 *  Null when only this framing covers the object, or it is not among them. */
export function coverageFor(cover: Array<{ key: string | null; label: string; framing: CoverageFraming }>, key: string | null, layout: DirectorLayout): ShapeReading | null {
  const index = cover.findIndex(entry => entry.key === key);
  if (cover.length < 2 || index < 0) return null;
  const read = readCoverage(cover.map((entry, f) => f === index ? { ...entry.framing, layout } : entry.framing));
  const name = (panel: CoveragePanel) => panel.framing === index ? panel.id : `${cover[panel.framing].label} ${panel.id}`;
  const groups = new Map<string, string>();
  read.groups.forEach((members, f) => members.forEach((group, k) => groups.set(name({ framing: f, id: (f === index ? layout : cover[f].framing.layout).panels[k].id }), name(group))));
  return {
    issues: read.issues.filter(issue => issue.kind === 'disconnected' || issue.panels.some(panel => panel.framing === index))
      .map(issue => ({ kind: issue.kind, panels: issue.panels.map(name), percent: issue.percent })),
    groups,
  };
}

/** Each panel's reach in panel sizes, from its true outline, for placing
 *  panels against it. */
export function framingBoxes(center: DirectorSkyPosition, positionAngle: number, panel: DirectorPanelSize, layout: DirectorLayout): BoxOf {
  const shot = new Map(shapesOf(center, positionAngle, panel, layout).map(shape => [shape.id, shape]));
  return placement => {
    const shape = placement.measured && shot.get(placement.id)
      ? shot.get(placement.id)!
      : turnedShape(placement.id, [placement.x * panel.width_degrees, placement.y * panel.height_degrees], panel.width_degrees, panel.height_degrees, turnOf(placement));
    const b = boundsOf(shape.corners);
    return [b[0] / panel.width_degrees, b[1] / panel.width_degrees, b[2] / panel.height_degrees, b[3] / panel.height_degrees] as Bounds;
  };
}

/** One-based bands of values, a new one where the next value is more than
 *  `tolerance` past the last: the core's rows and columns. */
function bands(values: number[], tolerance: number): number[] {
  const order = values.map((_, index) => index).sort((a, b) => values[a] - values[b]);
  const out = new Array<number>(values.length).fill(0);
  let band = 0; let last: number | null = null;
  for (const index of order) {
    if (last === null || values[index] - last > tolerance) band += 1;
    last = values[index];
    out[index] = band;
  }
  return out;
}

/** The same geometry `POST /framing/preview` returns, computed here so the
 *  rectangles follow the pointer: each panel placed on the tangent plane at
 *  the target, corners taken to the sky, then onto the view's plane; a shot
 *  panel where its frames were taken. Each panel's camera angle carries
 *  north's lean at its center. A footprint past the view plane's horizon has
 *  no view corners. The server recomputes this from the saved draft when it
 *  activates, so nothing depends on the browser's copy being kept. */
export function framingGeometry(request: DirectorFramingRequest): DirectorFramingPreview {
  const { center, panel, layout } = request;
  const angle = request.position_angle_degrees;
  const view = request.view;
  const toView = (position: DirectorSkyPosition): DirectorOffset | null => {
    if (!view) return null;
    const offset = offsetFrom(view.center, position);
    if (!offset) return null;
    const rad = (view.rotation_degrees * Math.PI) / 180;
    return [offset[0] * Math.cos(rad) - offset[1] * Math.sin(rad), offset[0] * Math.sin(rad) + offset[1] * Math.cos(rad)];
  };
  const withView = (id: string, middle: DirectorSkyPosition, corners: DirectorFootprint['corners']): DirectorFootprint => {
    const mapped = view ? corners.map(toView) : null;
    return { id, center: middle, corners, view_corners: mapped && mapped.every(Boolean) ? mapped as DirectorFootprint['view_corners'] : null };
  };
  const footprint = (id: string, at: DirectorOffset, size: DirectorPanelSize, turn: number): DirectorFootprint =>
    withView(id, deprojectFrom(center, at), rectangle(at, size, turn).map(corner => deprojectFrom(center, corner)) as DirectorFootprint['corners']);
  const { shapes, reading } = readFraming(center, angle, panel, layout);
  // Rows and columns from where each panel sits, in panel sizes.
  const places = shapes.map(shape => { const [x, y] = shapeCenter(shape); return [x / panel.width_degrees, y / panel.height_degrees]; });
  const rows = bands(places.map(place => -place[1]), 0.1);
  const columns = bands(places.map(place => place[0]), 0.1);
  const panels: DirectorFramingPreview['panels'] = layout.panels.map((placement, index) => {
    const turn = turnOf(placement);
    const shown = placement.measured
      ? withView(placement.id, placement.measured.center, measuredCorners(placement.measured))
      : footprint(placement.id, cameraDegreesToPlane(shapeCenter(shapes[index]), angle), panel, angle + turn);
    return {
      row: rows[index], column: columns[index], locked: placement.locked,
      position_angle_degrees: placement.measured ? placement.measured.position_angle_degrees : cameraAngleAt(center, shown.center, angle + turn),
      angle_offset_degrees: turn,
      ...(placement.measured ? { measured: placement.measured } : {}),
      ...shown,
    };
  });
  return {
    schema_version: 1,
    panels,
    overlays: request.overlays.map(overlay => footprint(overlay.id, [0, 0], overlay.size, overlay.position_angle_degrees)),
    extent: extentOf(shapes),
    view_center_offset: view ? toView(center) : null,
    issues: reading.issues,
  };
}

/** The survey tile behind the stage follows the viewport: a little wider
 *  than the view, at the viewport's own pixel density, so the picture is
 *  sharp across the whole stage. The tiles fetched before, and the wider
 *  ones fetched quietly, cover a pan or a zoom step until its tile lands. */
export const TILE_MARGIN = 1.3;
/** The most pixels the server renders along a side. */
export const TILE_MAX_PIXELS = 2048;
export const TILE_WIDTH = 2048;
export const TILE_HEIGHT = 1536;
export interface SkyTile { center: DirectorSkyPosition; fov: number }

/** The tile's pixels: the stage's device pixels across, times the margin,
 *  at the stage's shape, within what the server renders. Without a measured
 *  width the stage's logical size at two pixels per unit is assumed. */
export function tileSize(stage: Stage = DEFAULT_STAGE, devicePixelsAcross = stage.width * 2): { width: number; height: number } {
  const aspect = stage.height / stage.width;
  let width = Math.min(TILE_MAX_PIXELS, Math.max(64, Math.round(devicePixelsAcross * TILE_MARGIN)));
  let height = Math.round(width * aspect);
  if (height > TILE_MAX_PIXELS) { height = TILE_MAX_PIXELS; width = Math.round(height / aspect); }
  return { width, height: Math.max(64, height) };
}

export function tileFor(view: DirectorSkyPosition, viewFov: number): SkyTile {
  return { center: view, fov: Math.min(TILE_MAX_FOV, viewFov * TILE_MARGIN) };
}

/** Whether the view has left the tile's useful area: near an edge, or
 *  zoomed so far in that the tile's pixels would show. Then a new tile is
 *  needed at once, not after the pointer rests. */
export function viewLeftTile(tile: SkyTile, view: DirectorSkyPosition, viewFov: number, stage: Stage = DEFAULT_STAGE): boolean {
  const offset = stageProject(tile.center, view);
  if (!offset) return true;
  const reach = tile.fov / 2 - viewFov / 2;
  return Math.abs(offset[0]) > reach * 0.9 || Math.abs(offset[1]) > reach * 0.9 * (stage.height / stage.width) || viewFov > tile.fov * 0.95 || viewFov < tile.fov / 1.8;
}

/** The sky under a pixel of a survey tile: the server renders tiles
 *  stereographically about their own center, north up and east left. */
export function tilePixelToSky(tile: SkyTile, pixels: { width: number; height: number }, x: number, y: number): DirectorSkyPosition {
  const scale = tile.fov / pixels.width;
  return stageDeproject(tile.center, [(pixels.width / 2 - x) * scale, (pixels.height / 2 - y) * scale]);
}

/** The SVG matrix that lays a loaded tile on the stage. The tile's own
 *  plane is centered where it was fetched; the stage's is anchored on the
 *  target, so north on the tile leans against north on the stage once the
 *  view has panned at any declination. Fitting the tile's corners through
 *  the sky onto the stage carries that turn, and the scale and shift, in
 *  one transform, so the picture stays under the grid and the rectangle
 *  while the pointer moves; the settled view then gets a tile of its own. */
export function tileMatrix(tile: SkyTile, pixels: { width: number; height: number }, view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): string | null {
  return affineFrom(pixels.width, pixels.height, (x, y) => tilePixelToSky(tile, pixels, x, y), view, viewFov, stage);
}

export const THUMB_WIDTH = 320;
export const THUMB_HEIGHT = 240;

/** The width of sky a plan thumbnail shows: the whole mosaic with room around it. */
export function thumbnailFov(framing: { extent: DirectorPanelSize | null; panel: DirectorPanelSize | null }): number {
  const extent = framing.extent ?? framing.panel;
  const needed = extent ? Math.max(extent.width_degrees, (extent.height_degrees * THUMB_WIDTH) / THUMB_HEIGHT) : 1;
  return clampFov(Math.max(0.3, needed * 1.6));
}

export function clampFov(fov: number): number {
  return Math.min(MAX_VIEW_FOV, Math.max(MIN_VIEW_FOV, fov));
}

export function formatRaHours(raDegrees: number): string {
  const hours = raDegrees / 15;
  const h = Math.floor(hours);
  const m = Math.floor((hours - h) * 60);
  const s = ((hours - h) * 60 - m) * 60;
  return `${String(h).padStart(2, '0')}h ${String(m).padStart(2, '0')}m ${s.toFixed(1).padStart(4, '0')}s`;
}

export function formatDec(decDegrees: number): string {
  const sign = decDegrees < 0 ? '−' : '+';
  const abs = Math.abs(decDegrees);
  const d = Math.floor(abs);
  const m = Math.floor((abs - d) * 60);
  const s = ((abs - d) * 60 - m) * 60;
  return `${sign}${String(d).padStart(2, '0')}° ${String(m).padStart(2, '0')}′ ${s.toFixed(0).padStart(2, '0')}″`;
}

export function formatDegrees(value: number): string {
  if (value >= 1) return `${value.toFixed(2)}°`;
  const minutes = value * 60;
  return minutes >= 1 ? `${minutes.toFixed(1)}′` : `${(minutes * 60).toFixed(0)}″`;
}

/** Stage pixels back to view offsets (east positive, up positive). */
export function fromStage(x: number, y: number, viewFov: number, stage: Stage = DEFAULT_STAGE): DirectorOffset {
  const scale = pixelScale(viewFov, stage);
  return [(stage.width / 2 - x) * scale, (stage.height / 2 - y) * scale];
}

/** A footprint's sky corners in window coordinates, or `null` when one is out of view. */
export function stageCorners(corners: DirectorSkyPosition[], view: StageView): DirectorOffset[] | null {
  const mapped = corners.map(corner => projectOn(view, corner));
  return mapped.every(Boolean) ? mapped as DirectorOffset[] : null;
}

/** Whether a stage point lies inside a footprint given by its stage corners. */
export function insidePolygon(point: [number, number], corners: DirectorOffset[], viewFov: number, stage: Stage = DEFAULT_STAGE): boolean {
  const pts = corners.map(c => toStage(c, viewFov, stage));
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const [xi, yi] = pts[i]; const [xj, yj] = pts[j];
    if ((yi > point[1]) !== (yj > point[1]) && point[0] < ((xj - xi) * (point[1] - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/** Position angle east of north of the direction from a center to a stage point. */
export function angleFromStage(center: [number, number], point: [number, number]): number {
  // Stage x grows west, stage y grows south; east of north is atan2(east, north).
  const east = center[0] - point[0];
  const north = center[1] - point[1];
  const angle = (Math.atan2(east, north) * 180) / Math.PI;
  return ((angle % 360) + 360) % 360;
}

/** Where the rotation handle sits: just past the top edge of the footprint along its up direction. */
export function handleOffset(positionAngle: number, halfHeightDegrees: number, marginDegrees: number): DirectorOffset {
  const r = halfHeightDegrees + marginDegrees;
  const rad = (positionAngle * Math.PI) / 180;
  return [r * Math.sin(rad), r * Math.cos(rad)];
}

/** The handle on the sky: laid out on the target's own tangent plane, where
 *  the camera angle is measured, then taken to the sky. Drawn through the
 *  stage projection like the rectangle, it stays on the rectangle's up
 *  direction wherever the view is looking; a handle placed in the view's
 *  plane would drift as the view panned away, since north tilts across a
 *  plane away from its center. */
export function handleSky(center: DirectorSkyPosition, positionAngle: number, halfHeightDegrees: number, marginDegrees: number): DirectorSkyPosition {
  return deprojectFrom(center, handleOffset(positionAngle, halfHeightDegrees, marginDegrees));
}

/** The framing's rotation handle on the sky: past the top edge of the whole
 *  mosaic, along the camera's up direction, a few per cent of the view out. */
export function rotationHandleSky(state: FramingState, extent: DirectorPanelSize, viewFov = state.viewFov): DirectorSkyPosition {
  // Above the top edge of the highest panel, which for a grid is half the extent.
  const top = state.panel && state.layout.panels.length > 0
    ? Math.max(...shapesOf(state.center, state.positionAngle, state.panel, state.layout).flatMap(shape => shape.corners.map(corner => corner[1])))
    : extent.height_degrees / 2;
  return handleSky(state.center, state.positionAngle, Number.isFinite(top) ? top : extent.height_degrees / 2, Math.max(0.02, viewFov * 0.03));
}

/** One panel's turn handle on the sky: past its top edge along its own up,
 *  a few per cent of the view out, on the framing's plane. `place` is the
 *  panel's middle in camera-frame degrees. */
export function panelHandleSky(center: DirectorSkyPosition, positionAngle: number, place: Point, turn: number, halfHeight: number, viewFov: number): DirectorSkyPosition {
  // Further out than the framing's own handle, clear of the + on its top edge.
  const reach = halfHeight + Math.max(0.03, viewFov * 0.05);
  const rad = (turn * Math.PI) / 180;
  // The panel's up in the camera frame, turned counter-clockwise by its turn.
  const tip: Point = [place[0] - reach * Math.sin(rad), place[1] + reach * Math.cos(rad)];
  return deprojectFrom(center, cameraDegreesToPlane(tip, positionAngle));
}

/** The turn from the framing's angle that points a panel's up from its
 *  middle (camera-frame degrees) at a sky position. */
export function turnToward(center: DirectorSkyPosition, positionAngle: number, place: Point, position: DirectorSkyPosition): number | null {
  const offset = offsetFrom(center, position);
  if (!offset) return null;
  const [x, y] = planeToCameraDegrees(offset, positionAngle);
  return (Math.atan2(-(x - place[0]), y - place[1]) * 180) / Math.PI;
}

/** How much wider than its footprint a fitted view is. */
export const FIT_MARGIN = 1.3;

/** The view width that shows every one of `points`, and the rotation handle
 *  when there is one, on a stage centered at `center` and turned by
 *  `rotation`, with room around them. Each point is taken as drawn, so a
 *  turned rectangle's corners count and not its unturned box. The handle
 *  stands further out as the view widens, so the width is stepped up
 *  from the corners' until it holds the handle too. `null` with no points. */
export function fitViewFov(points: DirectorSkyPosition[], handle: ((viewFov: number) => DirectorSkyPosition) | null, center: DirectorSkyPosition, rotation: number, stage: Stage = DEFAULT_STAGE): number | null {
  const view = viewAt(center, center, rotation);
  // A point x degrees from the center sits x / fov of the stage's width out,
  // and must stay inside half the stage's width and half its height.
  const needed = (positions: DirectorSkyPosition[]) => positions.reduce((widest, position) => {
    const at = projectOn(view, position);
    return at ? Math.max(widest, 2 * FIT_MARGIN * Math.abs(at[0]), (2 * FIT_MARGIN * Math.abs(at[1]) * stage.width) / stage.height) : widest;
  }, 0);
  let fov = needed(points);
  if (!(fov > 0)) return null;
  if (handle) for (let step = 0; step < 4; step += 1) fov = Math.max(fov, needed([handle(fov)]));
  return clampFov(fov);
}

/** The camera angle that points a footprint's up direction from its center
 *  at a sky position: east of north on the target's tangent plane. */
export function angleAt(center: DirectorSkyPosition, position: DirectorSkyPosition): number {
  const offset = offsetFrom(center, position);
  if (!offset) return 0;
  const angle = (Math.atan2(offset[0], offset[1]) * 180) / Math.PI;
  return ((angle % 360) + 360) % 360;
}

/** An SVG path through stage points, broken where a point leaves the sky
 *  or jumps across the stage; points far outside the stage are dropped. */
function stagePath(points: Array<DirectorOffset | null>, viewFov: number, stage: Stage, close = false): string {
  const w = stage.width;
  const h = stage.height;
  const inside = (p: [number, number]) => p[0] > -w && p[0] < 2 * w && p[1] > -h && p[1] < 2 * h;
  let d = '';
  let drawn = 0;
  let previous: [number, number] | null = null;
  for (const point of points) {
    const at = point ? toStage(point, viewFov, stage) : null;
    if (!at || !inside(at) || (previous && Math.hypot(at[0] - previous[0], at[1] - previous[1]) > w)) { previous = null; if (at && inside(at)) { d += `M${at[0].toFixed(1)} ${at[1].toFixed(1)}`; previous = at; drawn += 1; } continue; }
    d += previous ? `L${at[0].toFixed(1)} ${at[1].toFixed(1)}` : `M${at[0].toFixed(1)} ${at[1].toFixed(1)}`;
    previous = at;
    drawn += 1;
  }
  if (drawn < 2) return '';
  return close ? `${d}Z` : d;
}

const DEC_STEPS = [45, 30, 20, 10, 5, 2, 1, 0.5, 1 / 3, 1 / 6, 1 / 12, 1 / 30, 1 / 60];
const RA_STEP_HOURS = [6, 3, 2, 1, 0.5, 1 / 3, 1 / 6, 1 / 12, 1 / 30, 1 / 60, 1 / 120, 1 / 360];

/** Grid spacing for a view: about five lines across, right ascension
 *  spaced to look like declination at the view's latitude. */
export function gridSteps(viewFov: number, decDegrees: number): { raDegrees: number; decDegrees: number } {
  const want = trueWidth(viewFov) / 5;
  const dec = DEC_STEPS.find(step => step <= want) ?? DEC_STEPS[DEC_STEPS.length - 1];
  const cos = Math.max(0.1, Math.cos((decDegrees * Math.PI) / 180));
  const wantRa = dec / cos / 15;
  const raHours = RA_STEP_HOURS.find(step => step <= wantRa) ?? RA_STEP_HOURS[RA_STEP_HOURS.length - 1];
  return { raDegrees: raHours * 15, decDegrees: dec };
}

function formatRaLabel(raDegrees: number, stepDegrees: number): string {
  if (stepDegrees >= 0.25) return formatRaShort(raDegrees);
  const seconds = Math.round((((raDegrees % 360) + 360) % 360) / 15 * 3600);
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  return `${h}h ${String(m).padStart(2, '0')}m ${String(s).padStart(2, '0')}s`;
}

export interface Graticule {
  paths: Array<{ kind: 'ra' | 'dec'; d: string }>;
  labels: Array<{ kind: 'ra' | 'dec'; x: number; y: number; text: string }>;
}

/** The equatorial grid over the view, with labels down the left edge for
 *  declination and along the top for right ascension, clear of the survey
 *  chips and the scale readout at the bottom. */
export function framingGraticule(view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): Graticule {
  const center = deprojectOn(view, [0, 0]);
  const { raDegrees: raStep, decDegrees: decStep } = gridSteps(viewFov, center.dec_degrees);
  const reach = (trueWidth(viewFov) / 2) * Math.hypot(1, stage.height / stage.width) * 1.1;
  const decMin = Math.max(-90, center.dec_degrees - reach);
  const decMax = Math.min(90, center.dec_degrees + reach);
  const overPole = Math.abs(center.dec_degrees) + reach >= 88;
  const cos = Math.cos((center.dec_degrees * Math.PI) / 180);
  const halfRa = overPole ? 180 : Math.min(180, reach / Math.max(cos, 1e-3));
  const raStart = center.ra_degrees - halfRa;
  const raEnd = center.ra_degrees + halfRa;
  const paths: Graticule['paths'] = [];
  const labels: Graticule['labels'] = [];
  const samples = (from: number, to: number, n: number) => Array.from({ length: n + 1 }, (_, i) => from + ((to - from) * i) / n);
  const crossing = (points: Array<DirectorOffset | null>, axis: 0 | 1, edge: number) => {
    const pts = points.map(p => (p ? toStage(p, viewFov, stage) : null));
    for (let i = 1; i < pts.length; i += 1) {
      const a = pts[i - 1]; const b = pts[i];
      if (!a || !b) continue;
      if ((a[axis] - edge) * (b[axis] - edge) <= 0 && a[axis] !== b[axis]) {
        const t = (edge - a[axis]) / (b[axis] - a[axis]);
        const other = a[1 - axis] + t * (b[1 - axis] - a[1 - axis]);
        return axis === 0 ? [edge, other] as [number, number] : [other, edge] as [number, number];
      }
    }
    return null;
  };
  for (let dec = Math.ceil(decMin / decStep) * decStep; dec <= decMax + 1e-9; dec += decStep) {
    if (Math.abs(dec) >= 90 - 1e-9) continue;
    const points = samples(raStart, raEnd, 120).map(ra => projectOn(view, { ra_degrees: ra, dec_degrees: dec }));
    const d = stagePath(points, viewFov, stage);
    if (!d) continue;
    paths.push({ kind: 'dec', d });
    const at = crossing(points, 0, 6);
    if (at && at[1] > 14 && at[1] < stage.height - 6) labels.push({ kind: 'dec', x: 8, y: at[1] - 4, text: formatDecShort(dec) });
  }
  const meridians = Math.min(Math.round(360 / raStep), Math.ceil((raEnd - raStart) / raStep) + 1);
  // Meridians crowd toward a pole; a label is skipped when the last one is
  // still under it.
  const raLabelsAt: number[] = [];
  for (let i = 0; i < meridians; i += 1) {
    const ra = (Math.ceil(raStart / raStep) + i) * raStep;
    if (ra > raEnd + 1e-9 && !overPole) break;
    const points = samples(decMin, decMax, 96).map(dec => projectOn(view, { ra_degrees: ra, dec_degrees: dec }));
    const d = stagePath(points, viewFov, stage);
    if (!d) continue;
    paths.push({ kind: 'ra', d });
    const at = crossing(points, 1, 6);
    if (at && at[0] > 6 && at[0] < stage.width - 70 && raLabelsAt.every(x => Math.abs(x - at[0]) >= 70)) {
      raLabelsAt.push(at[0]);
      labels.push({ kind: 'ra', x: at[0] + 4, y: 18, text: formatRaLabel(ra, raStep) });
    }
  }
  return { paths, labels };
}

/** The screen angle, degrees clockwise from the stage's x axis, of a
 *  direction at a sky position given as a position angle east of north:
 *  a short step that way, taken to the sky and back onto the stage. */
export function stageAngleAt(view: StageView, at: DirectorSkyPosition, positionAngle: number, viewFov: number, stage: Stage = DEFAULT_STAGE): number | null {
  const here = projectOn(view, at);
  const rad = (positionAngle * Math.PI) / 180;
  const there = projectOn(view, deprojectFrom(at, [0.05 * Math.sin(rad), 0.05 * Math.cos(rad)]));
  if (!here || !there) return null;
  const a = toStage(here, viewFov, stage);
  const b = toStage(there, viewFov, stage);
  return (Math.atan2(b[1] - a[1], b[0] - a[0]) * 180) / Math.PI;
}

/** How many deep-sky marks get a name at this width: every one when zoomed
 *  in, a handful when the whole sky is up. */
export function markLabelBudget(viewFov: number): number {
  return viewFov <= 5 ? 400 : viewFov <= 15 ? 60 : viewFov <= 40 ? 30 : 12;
}

/** Where each backdrop layer starts to make sense: stars and figures are
 *  worth drawing once the survey image is no longer showing them better. */
export const STARS_FROM_FOV = 8;
export const FIGURES_FROM_FOV = 12;
export const NAMES_FROM_FOV = 20;
export const MILKY_WAY_FROM_FOV = 45;

export interface Backdrop {
  stars: Array<{ x: number; y: number; r: number }>;
  figures: string[];
  names: Array<{ x: number; y: number; text: string }>;
  milkyWay: string[];
}

/** The Sky view's chart under the framing: bright stars, constellation
 *  figures and names, and the Milky Way band, each once the view is wide
 *  enough for it. */
export function framingBackdrop(view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): Backdrop {
  const onStage = (p: [number, number]) => p[0] >= -20 && p[0] <= stage.width + 20 && p[1] >= -20 && p[1] <= stage.height + 20;
  const stars: Backdrop['stars'] = [];
  if (viewFov >= STARS_FROM_FOV) {
    for (const [ra, dec, mag] of BRIGHT_STARS) {
      const offset = projectOn(view, { ra_degrees: ra, dec_degrees: dec });
      if (!offset) continue;
      const at = toStage(offset, viewFov, stage);
      if (onStage(at)) stars.push({ x: at[0], y: at[1], r: Math.max(0.9, 5.5 - mag) * 0.85 + 0.8 });
    }
  }
  const figures = viewFov >= FIGURES_FROM_FOV
    ? Object.values(CONSTELLATION_LINES).map(figure => figure.map(polyline => stagePath(polyline.map(([ra, dec]) => projectOn(view, { ra_degrees: ra, dec_degrees: dec })), viewFov, stage)).join('')).filter(Boolean)
    : [];
  const names: Backdrop['names'] = [];
  if (viewFov >= NAMES_FROM_FOV) {
    for (const [name, ra, dec] of Object.values(CONSTELLATION_NAMES)) {
      const offset = projectOn(view, { ra_degrees: ra, dec_degrees: dec });
      if (!offset) continue;
      const at = toStage(offset, viewFov, stage);
      if (onStage(at)) names.push({ x: at[0], y: at[1], text: name });
    }
  }
  const milkyWay = viewFov >= MILKY_WAY_FROM_FOV
    ? galacticBandQuads(12).map(quad => stagePath(quad.map(([ra, dec]) => projectOn(view, { ra_degrees: ra, dec_degrees: dec })), viewFov, stage, true)).filter(d => d && !d.includes('M', 1))
    : [];
  return { stars, figures, names, milkyWay };
}

/** The layers worth a chip on the sky: the DSS2 colour plates N.I.N.A. starts
 *  from and every narrowband layer, under short names. The View select
 *  still offers the whole list. */
const CHIP_LABELS: Record<string, string> = {
  dss2_color: 'DSS2', finkbeiner_halpha: 'Hα Finkbeiner', nsns_halpha: 'Hα NSNS', nsns_oiii: 'O III NSNS',
  nsns_ohs: 'SHO NSNS', nsns_halpha_continuum: 'Hα + continuum', nsns_dr01_color: 'NSNS colour',
};
export function chipSurveys(surveys: DirectorSurvey[]): Array<{ survey: DirectorSurvey; label: string }> {
  // Offline maps first: they answer at once and need no network.
  const offline = surveys.filter(survey => survey.offline).map(survey => ({ survey, label: survey.name }));
  const online = surveys
    .filter(survey => !survey.offline && (survey.id === 'dss2_color' || survey.kind === 'narrowband'))
    .map(survey => ({ survey, label: CHIP_LABELS[survey.id] ?? survey.name.replace(/^Northern Sky Narrowband Survey /, 'NSNS ').replace(/ composite$/i, '') }));
  return [...offline, ...online];
}

/** The layer a new framing starts on: the offline DSS map when the server
 *  has one, else the online DSS2 colour plates. */
export function defaultSurveyId(surveys: DirectorSurvey[] | undefined, fallback: string): string {
  return surveys?.find(survey => survey.offline && survey.kind === 'broadband')?.id ?? fallback;
}

/** The layer to draw for a saved or chosen survey: the offline map that
 *  stands in for it when the server has one (it answers at once and needs
 *  no network), the survey itself when it is listed, and otherwise the
 *  default, for a draft saved against a map this server no longer holds. */
export function preferredSurveyId(surveyId: string, surveys: DirectorSurvey[] | undefined, fallback: string): string {
  if (!surveys) return surveyId;
  const offline = surveys.find(survey => survey.offline && survey.stands_in_for === surveyId);
  if (offline) return offline.id;
  if (surveys.some(survey => survey.id === surveyId)) return surveyId;
  return defaultSurveyId(surveys, fallback);
}
