import type { SkyPreview } from './types';

export interface DirectorStatus {
  protocol_version: number;
  enabled: boolean;
  instance_id: string | null;
  /** Whether this server may write into rig databases; without it Planning is read-only over the catalogs. */
  database_management: boolean;
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
  /** The Sync peer holding this rig's real database; null when it is on this server. */
  peer_id: string | null;
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
  peer_id: string | null;
}

export interface DirectorSurvey {
  id: string;
  name: string;
  hips: string;
  kind: 'broadband' | 'narrowband' | 'panorama';
  bandpass: string;
  attribution: string;
  /** Rendered from N.I.N.A. offline sky map tiles on the server; no network. */
  offline?: boolean;
  /** For an offline map, the online survey it is a local copy of; the framing view prefers it. */
  stands_in_for?: string | null;
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
/** One rig framed on its own over the shared target: its grid, an angle of its own or the shared one, and a size set by hand or its own field. */
export interface DirectorRigFraming { rig_id: string; center: DirectorSkyPosition | null; position_angle_degrees: number | null; mosaic: DirectorMosaic; panel: DirectorPanelSize | null }
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
  /** Absent in drafts saved before rigs could be framed on their own. */
  rig_framings?: DirectorRigFraming[];
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
export interface DirectorMoonPolicy {
  enabled: boolean;
  separation_degrees: number;
  width_days: number;
  relax_degrees_per_degree: number;
  relax_min_altitude_degrees: number;
  relax_max_altitude_degrees: number;
  moon_down: boolean;
}
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
  moon?: DirectorMoonPolicy;
  bandpass: DirectorBandpass;
}
export interface DirectorTemplateList { catalog_slug: string; catalog_name: string; rig: DirectorIdentity | null; templates: DirectorTemplate[] }
/** A template in Director's own library: settings any rig can shoot with; activation writes it into a rig database that lacks it. */
export interface DirectorLibraryTemplate {
  id: string;
  /** 0 until first saved. */
  revision: number;
  name: string;
  filter_name: string;
  gain: number | null;
  offset: number | null;
  bin: number | null;
  readout_mode: number | null;
  default_exposure_seconds: number;
  moon?: DirectorMoonPolicy;
  updated_at_ms: number;
  bandpass: DirectorBandpass;
}
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
  moon?: DirectorMoonPolicy;
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

/** `adopt` takes over a row already in the rig's database instead of
 *  adding a twin; `keep` is a plan on these targets activation leaves as
 *  it is. */
export type DirectorActivationAction = 'create' | 'update' | 'unchanged' | 'adopt' | 'keep';
export interface DirectorActivationChange { kind: 'project' | 'target' | 'template' | 'plan'; action: DirectorActivationAction; name: string; detail: string }
/** A rig on another PSF Guard: its rows go there by Sync once Apply has committed them here. */
export interface DirectorActivationPush {
  peer_id: string;
  peer_name: string;
  applied: boolean;
  summary: Record<string, number>;
  error: string | null;
}
export interface DirectorActivationRig {
  rig: DirectorIdentity;
  catalog_slug: string | null;
  catalog_name: string;
  profile_id: string | null;
  changes: DirectorActivationChange[];
  warnings: string[];
  applied: boolean;
  push: DirectorActivationPush | null;
}
export interface DirectorActivationPushReport {
  project: DirectorIdentity;
  activation_revision: number;
  rigs: Array<{ rig: DirectorIdentity; catalog_name: string; push: DirectorActivationPush }>;
  warnings: string[];
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

/** One activated panel's latest stack, placed by its plate solve when it has one. */
export interface DirectorMosaicPanel {
  panel_id: string;
  rig: DirectorIdentity;
  catalog_slug: string | null;
  catalog_name: string;
  target_guid: string;
  target_id: number | null;
  target_name: string | null;
  progress: { desired: number; acquired: number; accepted: number } | null;
  status: 'ready' | 'unsolved' | 'no_stack' | 'missing_target' | 'missing_catalog';
  preview: SkyPreview | null;
}
export interface DirectorMosaicPreview {
  project: DirectorIdentity;
  activation_revision: number | null;
  framing_revision: number | null;
  framing_stale: boolean;
  panels: DirectorMosaicPanel[];
  warnings: string[];
}

/** One target of a linked project, with the frames its exposure plans ask for, have and have accepted. */
/** Frames graded rejected are counted apart; acquired minus accepted minus rejected is still pending. */
export interface DirectorTargetProgress { name: string; desired: number; acquired: number; accepted: number; rejected: number; center: DirectorSkyPosition | null; rotation_degrees: number | null }
export interface DirectorPlanLink {
  catalog_slug: string;
  catalog_name: string;
  rig: DirectorIdentity;
  source_project_guid: string;
  source_row_id: number | null;
  source_name: string | null;
  /** Target Scheduler's project state there: 0 draft, 1 active, 2 inactive, 3 closed. */
  source_state?: number | null;
  /** First and last capture of the project's frames there, Unix seconds. */
  earliest_capture_s?: number | null;
  latest_capture_s?: number | null;
  targets: DirectorTargetProgress[];
}
/** Enough of a saved framing for the plan list to draw its survey thumbnail. */
export interface DirectorFramingSummary {
  /** `draft`: saved in Director. `catalog`: the linked database's target stands in until a draft is saved. */
  source: 'draft' | 'catalog';
  revision: number;
  target_name: string;
  panels: number;
  panel_rig_id: string | null;
  center: DirectorSkyPosition;
  position_angle_degrees: number;
  panel: DirectorPanelSize | null;
  mosaic: DirectorMosaic;
  survey_id: string;
  extent: DirectorPanelSize | null;
}
export interface DirectorPlanRow {
  project: DirectorIdentity;
  links: DirectorPlanLink[];
  /** Frames across every linked database; null until some database holds a target. */
  progress: { desired: number; acquired: number; accepted: number; rejected: number; targets: number } | null;
  framing: DirectorFramingSummary | null;
  plan: { revision: number; objectives: number; rigs: number } | null;
  activation: { revision: number; applied_at_ms: number; rigs: number } | null;
}
export interface DirectorPlanList { rows: DirectorPlanRow[]; warnings: string[] }
/** What attaching a plan did: whose links moved, and which drafts came along because this plan had none. */
export interface DirectorAttached { into: DirectorIdentity; absorbed: DirectorIdentity; moved_links: number; framing_taken: boolean; plan_taken: boolean }
export interface DirectorRigStatus { rig_id: string; session_id: string; reported_at_ms: number; payload: Record<string, unknown>; received_at_ms: number }
export interface DirectorContact { at_ms: number; detail: string | null }
export type DirectorConnectivityState = 'online' | 'stale' | 'offline' | 'never';
/** One rig as the operator sees it: connectivity from server receipt times, the newest report, and what it is assigned. */
export interface DirectorRigStatusView {
  rig: DirectorIdentity;
  catalog_slug: string | null;
  catalog_name: string | null;
  status: DirectorRigStatus | null;
  status_age_ms: number | null;
  /** The report is older than ten minutes; history, not now. */
  status_stale: boolean;
  checkins: Array<{ rig_id: string; ledger_id: string; highest_contiguous: number; highest_seen: number; last_checkin_ms: number }>;
  contacts: { program_pull: DirectorContact | null; check_in: DirectorContact | null; status: DirectorContact | null };
  connectivity: { state: DirectorConnectivityState; last_contact_ms: number | null; age_ms: number | null };
  assignments: Array<{ project: DirectorIdentity; activation_revision: number; applied_at_ms: number }>;
  pending_receipts: number;
}

export interface DirectorResolvedName { query: string; name: string; ra_degrees: number; dec_degrees: number; source: string }
/** One local catalog name for the search dropdown, with what matched the typing. */
export interface DirectorNameSearchHit { name: string; common_name: string; kind: string; ra_degrees: number; dec_degrees: number; matched: string; source: string }
export interface DirectorNameSearch {
  query: string;
  local: { available: boolean; note?: string; items: DirectorNameSearchHit[] };
  online: DirectorResolvedName | null;
  online_state: 'skipped' | 'hit' | 'miss' | 'failed';
  online_note?: string;
  online_cached: boolean;
}

/** One catalog layer of sky marks: whether its catalog is on the server, and what it found. */
export interface DirectorMarkLayer<T> { available: boolean; note?: string; items: T[] }
export interface DirectorObjectMark {
  id: string; name: string; common_name: string; kind: string; ra_degrees: number; dec_degrees: number;
  mag: number | null; major_arcmin: number | null; minor_arcmin: number | null; position_angle_degrees: number | null; prominence: number;
}
export interface DirectorMinorBodyMark {
  name: string; kind: 'comet' | 'asteroid'; ra_degrees: number; dec_degrees: number; mag: number; distance_au: number;
  motion_arcsec_per_hour: number | null; direction_pa_degrees: number | null;
}
export interface DirectorSolarSystemMark { name: string; kind: 'sun' | 'moon' | 'planet'; ra_degrees: number; dec_degrees: number; distance_au: number; elongation_degrees: number }
export interface DirectorSkyMarks {
  hidden: string[];
  catalogs?: string[];
  at_ms: number;
  radius_degrees: number;
  objects: DirectorMarkLayer<DirectorObjectMark>;
  minor_bodies: DirectorMarkLayer<DirectorMinorBodyMark>;
  solar_system: DirectorSolarSystemMark[];
}
/** `catalogs` names the catalog prefixes to keep, comma-separated, and `hide` those to leave out; with neither the server hides PGC and HD. */
export interface DirectorSkyMarksQuery { ra: number; dec: number; fov: number; aspect: number; at: number; limit?: number; hide?: string; catalogs?: string }

export interface DirectorNightTarget { id: string; hours_up: number; hours_up_moon_down: number; hours_lost_to_meridian: number; transit_ms: number | null; max_altitude_degrees: number; min_moon_separation_degrees: number }
export interface DirectorNight {
  date: string;
  noon_ms: number;
  dusk_ms: number | null;
  dawn_ms: number | null;
  dark_hours: number;
  moon_illumination: number;
  moon_hours_up_in_dark: number;
  targets: DirectorNightTarget[];
}
export interface DirectorNightSample {
  t_ms: number;
  sun_altitude_degrees: number;
  moon_altitude_degrees: number;
  targets: Array<{ altitude_degrees: number; azimuth_degrees: number; horizon_altitude_degrees: number | null; allowed: boolean; meridian_blocked: boolean }>;
}
export interface DirectorRigFeasibility {
  rig: DirectorIdentity;
  catalog_name: string;
  site: DirectorSite;
  custom_horizon: boolean;
  limits: DirectorLimits;
  nights: DirectorNight[];
  curve: { night: DirectorNight; samples: DirectorNightSample[] };
  hours_needed: number | null;
  nights_to_complete: number | null;
  in_plan: boolean;
}
export interface DirectorFeasibility {
  center: DirectorSkyPosition;
  target_name: string;
  nights: number;
  rigs: DirectorRigFeasibility[];
  warnings: string[];
}
