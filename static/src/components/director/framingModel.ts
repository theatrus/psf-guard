import type { DirectorFramingDraft, DirectorFramingRequest, DirectorMosaic, DirectorOffset, DirectorPanelSize, DirectorRigProfileSummary, DirectorSkyPosition } from '../../api/directorTypes';
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
