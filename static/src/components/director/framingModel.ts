import type { DirectorFootprint, DirectorFramingDraft, DirectorFramingPreview, DirectorFramingRequest, DirectorMosaic, DirectorOffset, DirectorPanelSize, DirectorRigProfileSummary, DirectorSkyPosition, DirectorSurvey, DirectorRigFraming} from '../../api/directorTypes';
import type { SkyPreview } from '../../api/types';
import { formatDecShort, formatRaShort, galacticBandQuads, tanPixelToSky } from '../../utils/skyProjection';
import { BRIGHT_STARS, CONSTELLATION_LINES, CONSTELLATION_NAMES } from '../../data/skyBackdrop';

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
  mosaic: DirectorMosaic;
  panelRigId: string | null;
  panel: DirectorPanelSize | null;
  shownRigIds: string[];
  surveyId: string;
  viewCenter: DirectorSkyPosition;
  viewFov: number;
  /** Rigs framed on their own; every other rig shoots the shared framing. */
  rigFramings: DirectorRigFraming[];
}

export function stateFromDraft(draft: DirectorFramingDraft): FramingState {
  return {
    targetName: draft.target_name, center: draft.center, positionAngle: draft.position_angle_degrees,
    mosaic: draft.mosaic, panelRigId: draft.panel_rig_id, panel: draft.panel, shownRigIds: draft.shown_rig_ids,
    surveyId: draft.survey_id, viewCenter: draft.center, viewFov: draft.view_fov_degrees, rigFramings: draft.rig_framings ?? [],
  };
}

export function stateFromSeed(seed: FramingSeed, defaultSurvey: string): FramingState {
  return {
    targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees,
    mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, panelRigId: null, panel: null, shownRigIds: [],
    surveyId: defaultSurvey, viewCenter: seed.center, viewFov: 4, rigFramings: [],
  };
}

export function draftFromState(state: FramingState, projectId: string, revision: number): DirectorFramingDraft {
  return {
    project_id: projectId, revision, target_name: state.targetName.trim(), center: state.center,
    position_angle_degrees: state.positionAngle, mosaic: state.mosaic, panel_rig_id: state.panelRigId,
    panel: state.panel, shown_rig_ids: state.shownRigIds, survey_id: state.surveyId,
    view_fov_degrees: state.viewFov, updated_at_ms: 0, rig_framings: state.rigFramings,
  };
}

/** What one rig shoots, the way the server lays it out: its own framing when
 *  it has one (its field standing in for a size not set by hand), else the
 *  shared framing. Null when no panel size is known. */
export function rigLayout(state: FramingState, rigId: string, rigs: DirectorRigProfileSummary[]): { center: DirectorSkyPosition; positionAngle: number; panel: DirectorPanelSize; mosaic: DirectorMosaic; own: boolean } | null {
  const own = state.rigFramings.find(entry => entry.rig_id === rigId);
  if (own) {
    const panel = own.panel ?? panelForRig(rigs, rigId);
    return panel ? { center: own.center ?? state.center, positionAngle: own.position_angle_degrees ?? state.positionAngle, panel, mosaic: own.mosaic, own: true } : null;
  }
  return state.panel ? { center: state.center, positionAngle: state.positionAngle, panel: state.panel, mosaic: state.mosaic, own: false } : null;
}

/** The geometry of every rig framed on its own, for the stage. */
export function rigGeometries(state: FramingState, rigs: DirectorRigProfileSummary[]): Array<{ rigId: string; geometry: DirectorFramingPreview; layout: NonNullable<ReturnType<typeof rigLayout>> }> {
  return state.rigFramings.flatMap(own => {
    const layout = rigLayout(state, own.rig_id, rigs);
    if (!layout) return [];
    return [{ rigId: own.rig_id, layout, geometry: framingGeometry({ center: layout.center, position_angle_degrees: layout.positionAngle, panel: layout.panel, mosaic: layout.mosaic, overlays: [], view: { center: state.viewCenter, rotation_degrees: 0 } }) }];
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
    center: state.center, position_angle_degrees: state.positionAngle, panel: state.panel, mosaic: state.mosaic,
    // A rig framed on its own is drawn as its own grid, not as an overlay.
    overlays: state.shownRigIds.filter(id => !state.rigFramings.some(own => own.rig_id === id)).flatMap(id => {
      const size = panelForRig(rigs, id);
      return size ? [{ id, size, position_angle_degrees: state.positionAngle }] : [];
    }),
    view: { center: state.viewCenter, rotation_degrees: 0 },
  };
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

/** The mosaic's step between panel centers along the camera axes. */
function mosaicStep(panel: DirectorPanelSize, mosaic: DirectorMosaic): [number, number] {
  const keep = 1 - mosaic.overlap_percent / 100;
  return [panel.width_degrees * keep, panel.height_degrees * keep];
}

/** The same geometry `POST /framing/preview` returns, computed here so the
 *  rectangle follows the pointer: panels laid out on the tangent plane at the
 *  target, corners taken to the sky, then onto the view's plane. A footprint
 *  past the view plane's horizon has no view corners. The server recomputes
 *  this from the saved draft when it activates, so nothing depends on the
 *  browser's copy being kept. */
export function framingGeometry(request: DirectorFramingRequest): DirectorFramingPreview {
  const { center, panel, mosaic } = request;
  const view = request.view;
  const toView = (position: DirectorSkyPosition): DirectorOffset | null => {
    if (!view) return null;
    const offset = offsetFrom(view.center, position);
    if (!offset) return null;
    const rad = (view.rotation_degrees * Math.PI) / 180;
    return [offset[0] * Math.cos(rad) - offset[1] * Math.sin(rad), offset[0] * Math.sin(rad) + offset[1] * Math.cos(rad)];
  };
  const footprint = (id: string, at: DirectorOffset, size: DirectorPanelSize, angle: number): DirectorFootprint => {
    const corners = rectangle(at, size, angle).map(corner => deprojectFrom(center, corner)) as DirectorFootprint['corners'];
    const mapped = view ? corners.map(toView) : null;
    return { id, center: deprojectFrom(center, at), corners, view_corners: mapped && mapped.every(Boolean) ? mapped as DirectorFootprint['view_corners'] : null };
  };
  const [stepX, stepY] = mosaicStep(panel, mosaic);
  const rad = (request.position_angle_degrees * Math.PI) / 180;
  const up = [Math.sin(rad), Math.cos(rad)];
  const right = [-Math.cos(rad), Math.sin(rad)];
  const panels: DirectorFramingPreview['panels'] = [];
  for (let row = 0; row < mosaic.rows; row += 1) {
    // Row one is the top of the mosaic as the camera sees it.
    const dy = ((mosaic.rows - 1) / 2 - row) * stepY;
    for (let column = 0; column < mosaic.columns; column += 1) {
      const dx = (column - (mosaic.columns - 1) / 2) * stepX;
      const at: DirectorOffset = [dx * right[0] + dy * up[0], dx * right[1] + dy * up[1]];
      panels.push({ row: row + 1, column: column + 1, ...footprint(`r${row + 1}c${column + 1}`, at, panel, request.position_angle_degrees) });
    }
  }
  return {
    schema_version: 1,
    panels,
    overlays: request.overlays.map(overlay => footprint(overlay.id, [0, 0], overlay.size, overlay.position_angle_degrees)),
    extent: { width_degrees: panel.width_degrees + stepX * (mosaic.columns - 1), height_degrees: panel.height_degrees + stepY * (mosaic.rows - 1) },
    view_center_offset: view ? toView(center) : null,
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
