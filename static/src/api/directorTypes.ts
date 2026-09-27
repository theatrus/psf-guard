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
