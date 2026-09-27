import type { DirectorFramingDraft, DirectorFramingRequest, DirectorMosaic, DirectorOffset, DirectorPanelSize, DirectorRigProfileSummary, DirectorSkyPosition } from '../../api/directorTypes';

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
