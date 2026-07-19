import type { OverlaySolution } from '@seiza/astro-overlay';

export interface Project {
  id: number;
  profile_id: string;
  profile_name: string;
  name: string;
  display_name: string;
  description: string | null;
  has_files: boolean;
}

export interface Target {
  id: number;
  name: string;
  ra: number;
  dec: number;
  active: boolean;
  image_count: number;
  accepted_count: number;
  rejected_count: number;
  has_files: boolean;
}

export interface Image {
  id: number;
  project_id: number;
  project_name: string;
  project_display_name: string;
  target_id: number;
  target_name: string;
  acquired_date: number | null;
  filter_name: string | null;
  grading_status: number;
  reject_reason: string | null;
  metadata: Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  filesystem_path: string | null;
}

export interface StarInfo {
  x: number;
  y: number;
  hfr: number;
  fwhm: number;
  brightness: number;
  eccentricity: number;
}

export interface StarDetectionResponse {
  detected_stars: number;
  average_hfr: number;
  average_fwhm: number;
  stars: StarInfo[];
}

export type AstrometryAnalysisStatus = 'unavailable' | 'catalog_only' | 'solved' | 'failed';
export type AstrometrySolveMode = 'embedded_wcs' | 'hinted' | 'blind';
export type AstrometryCatalogScope =
  | 'embedded_footprint'
  | 'solved_footprint'
  | 'estimated_field'
  | 'nearby_target';

export interface AstrometryCoordinateSource {
  ra_deg: number;
  dec_deg: number;
  source: string;
  header_keywords?: string[];
}

export interface CatalogHit {
  stable_id: string;
  source: string;
  aliases: string[];
  parent_ids: string[];
  alternate_ids: string[];
  alternate_sources: string[];
  name: string;
  common_name: string;
  kind: string;
  mag: number | null;
  major_arcmin: number | null;
  minor_arcmin: number | null;
  position_angle_deg: number | null;
  ra_deg: number;
  dec_deg: number;
  center_inside: boolean;
  extent_only: boolean;
  distance_from_center_deg: number;
  predicted_prominence: number;
}

export interface PointingResult {
  expected_ra_deg: number;
  expected_dec_deg: number;
  east_offset_arcsec: number;
  north_offset_arcsec: number;
  separation_arcsec: number;
  target_in_frame: boolean;
  target_edge_margin_px?: number;
}

export interface AstrometryAnalysis {
  image_id: number;
  status: AstrometryAnalysisStatus;
  mode?: AstrometrySolveMode;
  hint_source?: AstrometryCoordinateSource;
  expected_source?: AstrometryCoordinateSource;
  solution?: OverlaySolution;
  catalog_hits: CatalogHit[];
  catalog_scope?: AstrometryCatalogScope;
  catalog_radius_deg?: number;
  pointing?: PointingResult;
  solver_provenance?: {
    seiza_version: string;
    detection_backend: string;
    star_catalog: { name: string; path: string; format: string; size_bytes: number };
    blind_index?: { name: string; path: string; format: string; size_bytes: number };
  };
  solve_attempt?: {
    outcome: 'solved' | 'no_match' | 'insufficient_stars' | 'decode_error' | 'unsupported_image' | 'resource_unavailable' | 'cancelled' | 'internal_error';
    modes_attempted: AstrometrySolveMode[];
    detected_stars?: number;
    duration_ms: number;
    image_quality_evidence: boolean;
    cacheable: boolean;
  };
  source_fingerprint: {
    canonical_path: string;
    size_bytes: number;
    modified_unix_seconds: number;
    modified_subsec_nanos?: number;
  };
  computed_at: number;
  error?: string;
}

export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
  status?: 'ready' | 'loading' | 'refreshing';
}

export interface ImageQuery {
  project_id?: number;
  target_id?: number;
  status?: 'pending' | 'accepted' | 'rejected';
  limit?: number;
  offset?: number;
}

export interface UpdateGradeRequest {
  status: 'pending' | 'accepted' | 'rejected';
  reason?: string;
}

export interface PreviewOptions {
  size?: 'screen' | 'large' | 'original';
  stretch?: boolean;
  midtone?: number;
  shadow?: number;
  max_stars?: number;
}

// Readiness of an on-demand preview/annotated artifact (the server generates
// it asynchronously on a bounded interactive queue; the frontend batch-polls).
export type GenerationState = 'ready' | 'generating' | 'error';

export interface GenerationStatus {
  state: GenerationState;
  error?: string;
}

// Identifies one artifact for the batch generation-status poll. Mirrors the
// query params of getPreviewUrl / getAnnotatedUrl.
export interface PreviewDescriptor {
  imageId: number;
  kind: 'preview' | 'annotated';
  size: 'screen' | 'large' | 'original';
  stretch?: boolean;
  midtone?: number;
  shadow?: number;
  maxStars?: number;
}

export interface ServerInfo {
  version: string;
  cache_directory: string;
  /** Whether POST/PUT/DELETE /api/databases are accepted on this server. */
  allow_database_management: boolean;
}

/** One configured database, returned by /api/databases. */
export interface DatabaseSummary {
  id: string;
  name: string;
  database_path: string;
  image_directories: string[];
}

export interface FileCheckResponse {
  images_checked: number;
  files_found: number;
  files_missing: number;
  check_time_ms: number;
}

export interface DirectoryTreeResponse {
  total_files: number;
  unique_filenames: number;
  total_directories: number;
  age_seconds: number;
  build_time_ms: number;
  root_directory: string;
}

export const GradingStatus = {
  Pending: 0,
  Accepted: 1,
  Rejected: 2,
} as const;

export type GradingStatus = typeof GradingStatus[keyof typeof GradingStatus];

// Overview types
export interface DateRange {
  earliest?: number;
  latest?: number;
  span_days?: number;
}

export interface ProjectOverview {
  id: number;
  profile_id: string;
  profile_name: string;
  name: string;
  display_name: string;
  description?: string;
  has_files: boolean;
  target_count: number;
  total_images: number;
  accepted_images: number;
  rejected_images: number;
  pending_images: number;
  total_desired: number;
  files_found: number;
  files_missing: number;
  date_range: DateRange;
  filters_used: string[];
}

export interface TargetOverview {
  id: number;
  name: string;
  ra?: number;
  dec?: number;
  active: boolean;
  project_id: number;
  project_name: string;
  image_count: number;
  accepted_count: number;
  rejected_count: number;
  pending_count: number;
  total_desired: number;
  files_found: number;
  files_missing: number;
  has_files: boolean;
  date_range: DateRange;
  filters_used: string[];
  coordinates_display?: string;
}

export interface OverallStats {
  total_projects: number;
  active_projects: number;
  total_targets: number;
  active_targets: number;
  total_images: number;
  accepted_images: number;
  rejected_images: number;
  pending_images: number;
  total_desired: number;
  files_found: number;
  files_missing: number;
  unique_filters: string[];
  date_range: DateRange;
  recent_activity: RecentActivity[];
}

export interface RecentActivity {
  date: number;
  images_added: number;
  images_graded: number;
}

export interface CacheRefreshProgress {
  is_refreshing: boolean;
  stage: string;
  progress_percentage: number;
  elapsed_seconds: number | null;
  directories_total: number;
  directories_processed: number;
  current_directory_name: string | null;
  files_scanned: number;
  projects_total: number;
  projects_processed: number;
  current_project_name: string | null;
  targets_total: number;
  targets_processed: number;
  files_found: number;
  files_missing: number;
}

// Sequence analysis types
export interface SequenceAnalysisRequest {
  target_id: number;
  filter_name?: string;
  session_gap_minutes?: number;
  weight_star_count?: number;
  weight_hfr?: number;
  weight_eccentricity?: number;
  weight_snr?: number;
  weight_background?: number;
  weight_spatial?: number;
  weight_pointing?: number;
}

export interface SequenceAnalysisResponse {
  sequences: ScoredSequence[];
}

export interface ScoredSequence {
  target_id: number;
  target_name: string;
  filter_name: string;
  session_start?: number;
  session_end?: number;
  image_count: number;
  reference_values: ReferenceValues;
  images: ImageQualityResult[];
  summary: SequenceSummary;
}

export interface ImageQualityResult {
  image_id: number;
  quality_score: number;
  temporal_anomaly_score: number;
  category: string | null;
  flags?: string[];
  normalized_metrics: {
    star_count: number | null;
    hfr: number | null;
    eccentricity: number | null;
    snr: number | null;
    background: number | null;
    /** Spatial star coverage (1 = whole frame, 0 = half+ of grid cells dead).
     * Only populated when spatial metrics were computed from FITS files;
     * DB-metadata-only analysis leaves it null. Optional for older servers. */
    spatial_coverage?: number | null;
    /** Photometric transparency mapped to 0..1 (1 = nominal matched-star
     * flux, 0 = <=60% of the sequence reference). Populated after a spatial
     * scan; null otherwise. Optional for older servers. */
    transparency?: number | null;
    /** Pixel-derived pointing score. Missing until a quality scan runs. */
    pointing?: number | null;
  };
  pointing?: {
    pixel_solved: boolean;
    solve_failed: boolean;
    image_quality_evidence: boolean;
    expected_target: boolean;
    flags: string[];
    east_offset_arcsec?: number;
    north_offset_arcsec?: number;
    separation_arcsec?: number;
    field_fraction_offset?: number;
    reference_offset_arcsec?: number;
    /** Residual from the segment's own robust center, as a field fraction. */
    reference_field_fraction?: number;
    drift_rate_arcsec_per_hour?: number;
    matched_stars?: number;
    rms_arcsec?: number;
    error?: string;
  };
  regrade_reason?: string;
  details: string | null;
}

export interface SpatialScanProgress {
  running: boolean;
  stage: string;
  target_id: number | null;
  filter_name: string | null;
  total: number;
  processed: number;
  skipped_cached: number;
  spatial_processed: number;
  astrometry_processed: number;
  solved: number;
  solve_failed: number;
  operational_errors: number;
  errors: number;
  current_file: string | null;
  started_at: number | null;
  finished_at: number | null;
  last_error: string | null;
}

export interface SpatialScanStatus {
  /** POST: whether this request started a scan. GET: whether one is running. */
  started: boolean;
  progress: SpatialScanProgress;
  /** Images with cached spatial metrics in this database. */
  cached_count: number;
}

export interface SpatialScanRequest {
  target_id: number;
  filter_name?: string;
  force?: boolean;
  force_spatial?: boolean;
  force_astrometry?: boolean;
}

export interface SequenceSummary {
  excellent_count: number;
  good_count: number;
  fair_count: number;
  poor_count: number;
  bad_count: number;
  cloud_events_detected: number;
  focus_drift_detected: boolean;
  tracking_issues_detected: boolean;
  out_of_target_count: number;
  plate_solve_failed_count: number;
}

export interface ReferenceValues {
  best_star_count?: number;
  best_hfr?: number;
  best_eccentricity?: number;
  best_snr?: number;
  best_background?: number;
}

export interface ImageQualityResponse {
  image_id: number;
  quality?: ImageQualityResult;
  sequence_target_id?: number;
  sequence_filter_name?: string;
  sequence_image_count?: number;
  reference_values?: ReferenceValues;
}
