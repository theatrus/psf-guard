import type { DirectorFootprint, DirectorFramingDraft, DirectorFramingPreview, DirectorFramingRequest, DirectorMosaic, DirectorOffset, DirectorPanelSize, DirectorRigProfileSummary, DirectorSkyPosition, DirectorSurvey } from '../../api/directorTypes';
import type { SkyPreview } from '../../api/types';
import { tanPixelToSky } from '../../utils/skyProjection';

/** The image the view asks the server for, in pixels; CSS scales it down. */
export const STAGE_WIDTH = 1024;
export const STAGE_HEIGHT = 768;
export const MIN_VIEW_FOV = 0.05;
export const MAX_VIEW_FOV = 40;

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
}

export function stateFromDraft(draft: DirectorFramingDraft): FramingState {
  return {
    targetName: draft.target_name, center: draft.center, positionAngle: draft.position_angle_degrees,
    mosaic: draft.mosaic, panelRigId: draft.panel_rig_id, panel: draft.panel, shownRigIds: draft.shown_rig_ids,
    surveyId: draft.survey_id, viewCenter: draft.center, viewFov: draft.view_fov_degrees,
  };
}

export function stateFromSeed(seed: FramingSeed, defaultSurvey: string): FramingState {
  return {
    targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees,
    mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, panelRigId: null, panel: null, shownRigIds: [],
    surveyId: defaultSurvey, viewCenter: seed.center, viewFov: 4,
  };
}

export function draftFromState(state: FramingState, projectId: string, revision: number): DirectorFramingDraft {
  return {
    project_id: projectId, revision, target_name: state.targetName.trim(), center: state.center,
    position_angle_degrees: state.positionAngle, mosaic: state.mosaic, panel_rig_id: state.panelRigId,
    panel: state.panel, shown_rig_ids: state.shownRigIds, survey_id: state.surveyId,
    view_fov_degrees: state.viewFov, updated_at_ms: 0,
  };
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
    overlays: state.shownRigIds.flatMap(id => {
      const size = panelForRig(rigs, id);
      return size ? [{ id, size, position_angle_degrees: state.positionAngle }] : [];
    }),
    view: { center: state.viewCenter, rotation_degrees: 0 },
  };
}

/** Degrees per stage pixel. */
export function pixelScale(viewFov: number): number {
  return viewFov / STAGE_WIDTH;
}

/** North up, east left: a positive east offset moves left on the stage. */
export function toStage(offset: DirectorOffset, viewFov: number): [number, number] {
  const scale = pixelScale(viewFov);
  return [STAGE_WIDTH / 2 - offset[0] / scale, STAGE_HEIGHT / 2 - offset[1] / scale];
}

export function polygonPoints(corners: DirectorOffset[], viewFov: number): string {
  return corners.map(corner => toStage(corner, viewFov).map(v => v.toFixed(1)).join(',')).join(' ');
}

/** Where the mouse points on the sky, from stage pixels. Small-angle, which is
 *  what a hand needs; the server's preview keeps the exact geometry. */
export function skyAtStage(view: DirectorSkyPosition, viewFov: number, x: number, y: number): DirectorSkyPosition {
  const scale = pixelScale(viewFov);
  const xi = (STAGE_WIDTH / 2 - x) * scale;
  const eta = (STAGE_HEIGHT / 2 - y) * scale;
  return moveBy(view, xi, eta);
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
 *  corners go through its TAN solution to the sky, then onto the view plane.
 *  Fitted to three corners, which is exact for the affine a preview needs at
 *  framing scales. `null` when any corner leaves the plane. */
export function stackMatrix(preview: SkyPreview, view: DirectorSkyPosition, viewFov: number): string | null {
  if (!preview.wcs || preview.width <= 0 || preview.height <= 0) return null;
  const { width, height, wcs } = preview;
  const points = ([[0, 0], [width, 0], [0, height]] as const).map(([x, y]) => {
    const [ra, dec] = tanPixelToSky(wcs, x, y);
    const offset = offsetFrom(view, { ra_degrees: ra, dec_degrees: dec });
    return offset ? toStage(offset, viewFov) : null;
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
export function fromStage(x: number, y: number, viewFov: number): DirectorOffset {
  const scale = pixelScale(viewFov);
  return [(STAGE_WIDTH / 2 - x) * scale, (STAGE_HEIGHT / 2 - y) * scale];
}

/** Whether a stage point lies inside a footprint given by its view corners. */
export function insidePolygon(point: [number, number], corners: DirectorOffset[], viewFov: number): boolean {
  const pts = corners.map(c => toStage(c, viewFov));
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

/** The layers worth a chip on the sky: the DSS2 colour plates N.I.N.A. starts
 *  from and every narrowband layer, under short names. The View select
 *  still offers the whole list. */
const CHIP_LABELS: Record<string, string> = {
  dss2_color: 'DSS2', finkbeiner_halpha: 'Hα Finkbeiner', nsns_halpha: 'Hα NSNS', nsns_oiii: 'O III NSNS',
  nsns_ohs: 'SHO NSNS', nsns_halpha_continuum: 'Hα + continuum', nsns_dr01_color: 'NSNS colour',
};
export function chipSurveys(surveys: DirectorSurvey[]): Array<{ survey: DirectorSurvey; label: string }> {
  return surveys
    .filter(survey => survey.id === 'dss2_color' || survey.kind === 'narrowband')
    .map(survey => ({ survey, label: CHIP_LABELS[survey.id] ?? survey.name.replace(/^Northern Sky Narrowband Survey /, 'NSNS ').replace(/ composite$/i, '') }));
}
