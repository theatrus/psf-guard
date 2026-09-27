export interface DirectorStatus {
  protocol_version: number;
  enabled: boolean;
  instance_id: string | null;
  acquisition_available: boolean;
}

export type DirectorCollection = 'projects' | 'sites' | 'rigs';

export interface DirectorIdentity {
  id: string;
  name: string;
  revision: number;
}

export interface DirectorIdentityPage {
  items: DirectorIdentity[];
  next_after: string | null;
}

export interface DirectorCatalogIdentity { id: string; origin_instance_id: string }
export interface DirectorMapping {
  catalog_id: string;
  source_project_guid: string;
  source_profile_id: string;
  project_id: string;
  rig_id: string;
}
export interface DirectorDiscovery {
  catalog_slug: string;
  catalog_name: string;
  catalog_identity: DirectorCatalogIdentity | null;
  snapshot_digest: string;
  evidence: {
    projects: Array<{ source_row_id: number; source_project_guid: string | null; source_profile_id: string | null; name: string | null; issues: string[] }>;
    profiles: Array<{ source_profile_id: string; project_count: number }>;
  };
}
export interface DirectorMappingPage {
  catalog_identity: DirectorCatalogIdentity | null;
  rig: DirectorIdentity | null;
  items: DirectorMapping[];
  next_after: string | null;
}
export interface DirectorRigPlan { catalog_id: string }
export interface DirectorRigReport {
  binding: { catalog: DirectorCatalogIdentity; rig: DirectorIdentity };
  preview_digest: string;
  applied: boolean;
}
export interface DirectorAdoptionPlan { catalog_id: string; mappings: DirectorMapping[] }
export interface DirectorAdoptionReport {
  catalog_identity: DirectorCatalogIdentity;
  preview_digest: string;
  applied: boolean;
  mappings: Array<{ mapping: DirectorMapping; source_name: string; project: DirectorIdentity; rig: DirectorIdentity }>;
}

export type DirectorSource = { kind: 'manual' } | { kind: 'frame_headers'; file_name: string } | { kind: 'plugin' };
export interface DirectorReported<T> { value: T; source: DirectorSource; reported_at_ms: number }
export type DirectorRotation = { mode: 'fixed'; angle_degrees: number } | { mode: 'manual'; angle_degrees: number } | { mode: 'rotator' };
export interface DirectorOptics {
  sensor_width_px: number;
  sensor_height_px: number;
  pixel_size_um: number;
  focal_length_mm: number;
  aperture_mm: number | null;
  rotation: DirectorRotation;
}
export interface DirectorFieldOfView { width_degrees: number; height_degrees: number; pixel_scale_arcsec: number; focal_ratio: number | null }
export interface DirectorSite { latitude_degrees: number; longitude_degrees: number; elevation_meters: number }
export type DirectorHorizon = { mode: 'fixed_minimum' } | { mode: 'custom'; points: Array<{ azimuth_degrees: number; altitude_degrees: number }> };
export interface DirectorSkyQuality { bortle_class: number; sqm_mag_per_arcsec2: number | null }
export interface DirectorLimits {
  minimum_altitude_degrees: number;
  maximum_altitude_degrees: number;
  meridian_exclusion: { before_ms: number; after_ms: number };
}
export interface DirectorRigProfile {
  rig_id: string;
  revision: number;
  optics: DirectorReported<DirectorOptics> | null;
  site: DirectorReported<DirectorSite> | null;
  horizon: DirectorReported<DirectorHorizon> | null;
  sky_quality: DirectorReported<DirectorSkyQuality> | null;
  limits: DirectorReported<DirectorLimits>;
  /** Only the N.I.N.A. plugin reports this; the form never edits it. */
  configuration: DirectorReported<unknown> | null;
  updated_at_ms: number;
}
export interface DirectorRigProfileView {
  rig: DirectorIdentity;
  profile: DirectorRigProfile;
  field_of_view: DirectorFieldOfView | null;
  defaults: {
    optics: DirectorReported<DirectorOptics> | null;
    site: DirectorReported<DirectorSite> | null;
    field_of_view: DirectorFieldOfView | null;
  };
}
export interface DirectorEdited<T> { value: T; source: DirectorSource }
export interface DirectorRigProfileEdit {
  expected_revision: number;
  optics: DirectorEdited<DirectorOptics> | null;
  site: DirectorEdited<DirectorSite> | null;
  horizon: DirectorEdited<DirectorHorizon> | null;
  sky_quality: DirectorEdited<DirectorSkyQuality> | null;
  limits: DirectorEdited<DirectorLimits>;
}

export interface DirectorSurvey {
  id: string;
  name: string;
  hips: string;
  kind: 'broadband' | 'narrowband' | 'panorama';
  bandpass: string;
  attribution: string;
}
export interface DirectorSkyPosition { ra_degrees: number; dec_degrees: number }
export interface DirectorPanelSize { width_degrees: number; height_degrees: number }
export interface DirectorMosaic { rows: number; columns: number; overlap_percent: number }
export interface DirectorFramingRequest {
  center: DirectorSkyPosition;
  position_angle_degrees: number;
  panel: DirectorPanelSize;
  mosaic: DirectorMosaic;
  overlays: Array<{ id: string; size: DirectorPanelSize; position_angle_degrees: number }>;
  view: { center: DirectorSkyPosition; rotation_degrees: number } | null;
}
/** Tangent-plane degrees from the view center: east positive, view up positive. */
export type DirectorOffset = [number, number];
export interface DirectorFootprint {
  id: string;
  center: DirectorSkyPosition;
  corners: [DirectorSkyPosition, DirectorSkyPosition, DirectorSkyPosition, DirectorSkyPosition];
  view_corners: [DirectorOffset, DirectorOffset, DirectorOffset, DirectorOffset] | null;
}
export interface DirectorFramingPreview {
  schema_version: number;
  panels: Array<DirectorFootprint & { row: number; column: number }>;
  overlays: DirectorFootprint[];
  extent: DirectorPanelSize;
  view_center_offset: DirectorOffset | null;
}
export interface DirectorFramingDraft {
  project_id: string;
  revision: number;
  target_name: string;
  center: DirectorSkyPosition;
  position_angle_degrees: number;
  mosaic: DirectorMosaic;
  panel_rig_id: string | null;
  panel: DirectorPanelSize | null;
  shown_rig_ids: string[];
  survey_id: string;
  view_fov_degrees: number;
  updated_at_ms: number;
}
export interface DirectorFramingDraftView { project: DirectorIdentity; draft: DirectorFramingDraft | null }
export interface DirectorRigProfileSummary {
  rig: DirectorIdentity;
  catalog_slug: string;
  catalog_name: string;
  profile: DirectorRigProfile | null;
  field_of_view: DirectorFieldOfView | null;
  /** Starting exposure lengths for this rig's optics and sky. */
  default_exposure_seconds: { broadband: number; narrowband: number };
}
export interface DirectorCutoutRequest {
  survey: string;
  ra: number;
  dec: number;
  fov: number;
  width: number;
  height: number;
  rotation: number;
}
export type DirectorCutoutResult =
  | { state: 'ready'; blob: Blob }
  | { state: 'generating' }
  | { state: 'failed'; error: string };

export interface DirectorBandpass { id: string; name: string; kind: 'broadband' | 'narrowband' }
export interface DirectorTemplate {
  id: number;
  guid: string | null;
  profile_id: string;
  name: string;
  filter_name: string;
  gain: number | null;
  offset: number | null;
  bin: number | null;
  readout_mode: number | null;
  default_exposure: number;
  bandpass: DirectorBandpass;
}
export interface DirectorTemplateList { catalog_slug: string; catalog_name: string; rig: DirectorIdentity | null; templates: DirectorTemplate[] }
export type DirectorGoal = { kind: 'hours'; value: number } | { kind: 'frames'; value: number };
export interface DirectorObjective { id: string; bandpass_id: string; purpose: string; goal: DirectorGoal; priority: number }
export interface DirectorTemplateChoice {
  template_guid: string | null;
  template_id: number | null;
  name: string;
  filter_name: string;
  gain: number | null;
  offset: number | null;
  bin: number | null;
  readout_mode: number | null;
}
export interface DirectorContribution {
  id: string;
  objective_id: string;
  rig_id: string;
  template: DirectorTemplateChoice;
  exposure_seconds: number;
  panel_ids: string[];
  enabled: boolean;
}
export interface DirectorPlanDraft {
  project_id: string;
  revision: number;
  objectives: DirectorObjective[];
  contributions: DirectorContribution[];
  updated_at_ms: number;
}
export interface DirectorPlanView { project: DirectorIdentity; plan: DirectorPlanDraft | null }

export type DirectorActivationAction = 'create' | 'update' | 'unchanged';
export interface DirectorActivationChange { kind: 'project' | 'target' | 'plan'; action: DirectorActivationAction; name: string; detail: string }
export interface DirectorActivationRig {
  rig: DirectorIdentity;
  catalog_slug: string | null;
  catalog_name: string;
  profile_id: string | null;
  changes: DirectorActivationChange[];
  warnings: string[];
  applied: boolean;
}
export interface DirectorActivationReport {
  project: DirectorIdentity;
  framing_revision: number;
  plan_revision: number;
  panels: number;
  rigs: DirectorActivationRig[];
  warnings: string[];
  preview_digest: string;
  applied: boolean;
  activation_revision: number | null;
}
export interface DirectorActivation {
  project_id: string;
  revision: number;
  framing_revision: number;
  plan_revision: number;
  coordinator_instance_id: string;
  applied_at_ms: number;
  rigs: Array<{ rig_id: string; catalog_id: string; project_guid: string; profile_id: string; targets: Array<{ panel_id: string; target_guid: string }>; plans: Array<{ contribution_id: string; objective_id: string; target_guid: string; exposureplan_guid: string; required_frames: number }> }>;
}
