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
  east_offset_arcsec?: number;
  north_offset_arcsec?: number;
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

export type BrightTrailRiskLevel = 'low' | 'possible' | 'high';

export interface PixelTrailSegment {
  start: { x: number; y: number };
  end: { x: number; y: number };
}

export interface PixelTrailAlignment {
  status: 'detected' | 'not_detected' | 'not_evaluated';
  not_evaluated_reason?: 'empty_path' | 'too_short' | 'insufficient_coverage';
  aligned_segments?: PixelTrailSegment[];
  start_normal_offset_px: number;
  end_normal_offset_px: number;
  mean_normal_offset_px: number;
  angle_delta_deg: number;
  contrast_adu: number;
  contrast_sigma: number;
  continuity: number;
  coverage: number;
  search_radius_px: number;
}

export interface SatelliteTrackPrediction {
  name: string;
  label: string;
  norad_id?: number;
  cospar_id?: string;
  association: 'predicted';
  element_epoch_utc: string;
  element_age_seconds: number;
  sample_interval_seconds: number;
  clipped_segments: [[number, number], [number, number]][];
  clipped_length_px: number;
  maximum_elevation_deg: number;
  minimum_range_km: number;
  maximum_sunlight_fraction: number;
  maximum_apparent_rate_arcsec_per_second?: number;
  maximum_pixel_rate_px_per_second?: number;
  bright_trail_risk: number;
  risk_level: BrightTrailRiskLevel;
  pixel_alignment?: PixelTrailAlignment;
}

export interface SatelliteAnalysis {
  image_id: number;
  association: 'predicted_not_pixel_detected' | 'predicted_pixel_checked' | 'predicted_with_pixel_alignment';
  seiza_version: string;
  seiza_satellites_version: string;
  pixel_alignment_version: number;
  image_width: number;
  image_height: number;
  exposure: {
    start_utc: string;
    end_utc: string;
    duration_seconds: number;
    latitude_deg: number;
    longitude_deg: number;
    altitude_m: number;
    provenance: string;
    header_keywords: string[];
  };
  catalog: {
    source: string;
    provider?: 'celes_trak_active' | 'seiza_mirror' | 'iau_sat_checker';
    state: 'configured' | 'fresh' | 'downloaded' | 'stale_fallback' | 'cached';
    cache_path?: string;
    size_bytes?: number;
    modified_unix_seconds?: number;
    retrieved_at?: string;
    query_epoch?: string;
    content_sha256?: string;
    warning?: string;
  };
  elements_considered: number;
  propagation_failures: number;
  stale_elements: number;
  tracks: SatelliteTrackPrediction[];
  risk: {
    track_count: number;
    potentially_bright_count: number;
    high_risk_count: number;
    maximum_bright_trail_risk: number;
    pixel_alignment_attempted: boolean;
    pixel_aligned_count: number;
    pixel_aligned_high_risk_count: number;
    reject_recommended: boolean;
  };
  pixel_alignment_error?: string;
  computed_at: number;
}

export interface SatelliteAnalysisStatus {
  analysis?: SatelliteAnalysis;
  orbital_elements_cached: boolean;
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

export type StackJobState = 'queued' | 'running' | 'completed' | 'failed';
export type StackGroupState = 'queued' | 'running' | 'ready' | 'skipped' | 'error';

export interface StackFrameDecision {
  image_id: number;
  disposition: 'excluded' | 'reference' | 'accepted' | 'rejected';
  reason: string | null;
  quality_score: number | null;
  matched_stars: number | null;
  registration_rms_pixels: number | null;
  registration_drift_pixels: number | null;
  overlap_fraction: number | null;
  integrated_fraction: number | null;
}

export interface StackInputImage {
  image_id: number;
  grading_status: number;
}

export interface StackGroupStatus {
  index: number;
  target_id: number;
  target_name: string;
  filter_name: string;
  state: StackGroupState;
  total_candidates: number;
  eligible_frames: number;
  quality_excluded: number;
  missing_files: number;
  processed_frames: number;
  accepted_frames: number;
  rejected_frames: number;
  output_channels: number;
  reference_image_id: number | null;
  total_exposure_seconds: number;
  preview_url: string | null;
  fits_url: string | null;
  error: string | null;
  input_images: StackInputImage[];
  frames: StackFrameDecision[];
}

export interface StackPreviewJob {
  schema_version: number;
  job_id: string;
  database_id: string;
  project_id: number;
  state: StackJobState;
  accepted_only: boolean;
  created_unix_seconds: number;
  artifact_revision: string;
  cache_version: number;
  stacking_version: string;
  groups: StackGroupStatus[];
  error: string | null;
}

export interface LatestStackPreviewGroup {
  job_id: string;
  artifact_revision: string;
  accepted_only: boolean;
  created_unix_seconds: number;
  group: StackGroupStatus;
}

export interface LatestStackPreviews {
  schema_version: number;
  database_id: string;
  project_id: number;
  updated_unix_seconds: number;
  groups: LatestStackPreviewGroup[];
}

export type StackStretchColorStrategy = 'linked' | 'unlinked' | 'luminance-preserving';

export type StackStretchModel =
  | { type: 'identity' }
  | { type: 'linear'; black: number; white: number }
  | { type: 'asinh'; black: number; white: number; strength: number }
  | {
      type: 'percentile-asinh';
      black_percentile: number;
      white_percentile: number;
      strength: number;
    }
  | { type: 'mtf'; shadows: number; midtone: number; highlights: number }
  | {
      type: 'ghs';
      stretch_factor: number;
      local_intensity: number;
      symmetry_point: number;
      protect_shadows: number;
      protect_highlights: number;
      black: number;
      white: number;
    }
  | { type: 'auto-mtf'; target_median: number; shadows_clip: number };

export interface StackStretchRequest {
  model: StackStretchModel;
  color_strategy: StackStretchColorStrategy;
}

export interface StackViewProcessingRequest extends StackStretchRequest {
  deconvolution?: StackDeconvolutionConfig | null;
}

export interface StackDeconvolutionConfig {
  psf_fwhm_pixels: number;
  iterations: number;
  amount: number;
  noise_fraction: number;
  max_correction: number;
}

export interface StackDeconvolutionChannelDiagnostics {
  input_flux: number;
  output_flux: number;
  input_peak: number;
  output_peak: number;
}

export interface StackDeconvolutionResult {
  config: StackDeconvolutionConfig;
  channels: StackDeconvolutionChannelDiagnostics[];
}

export interface StackStretchStatistics {
  min: number;
  max: number;
  median: number;
  mad: number;
  count: number;
}

export interface StackStretchPreview {
  schema_version: number;
  stretch_id: string;
  stretch_version: string;
  deconvolution_version: string | null;
  deconvolution_id: string | null;
  config: StackStretchRequest & { max_analysis_samples: number };
  resolved_plan: unknown;
  source_transfer: 'linear' | 'display_referred';
  input_range: { black: number; white: number } | null;
  linked_statistics: StackStretchStatistics;
  channel_statistics: Array<StackStretchStatistics | null>;
  luminance_statistics: StackStretchStatistics | null;
  deconvolution: StackDeconvolutionResult | null;
  preview_url: string;
  original_preview_url: string;
  fits_url: string | null;
}

export type StackColorRole = 'luminance' | 'red' | 'green' | 'blue' | 'ha' | 'oiii' | 'sii';
export type StackColorKind = 'rgb' | 'lrgb' | 'narrowband';
export type StackNarrowbandPalette =
  | 'sho'
  | 'soh'
  | 'hso'
  | 'hos'
  | 'osh'
  | 'ohs'
  | 'hoo'
  | 'foraxx-sho'
  | 'foraxx-hoo';

export interface StackColorSource {
  role: StackColorRole;
  filter_name: string;
  job_id: string;
  group_index: number;
  artifact_revision: string;
  accepted_frames: number;
}

export interface StackColorProcessing {
  background_extraction: StackBackgroundExtraction | null;
  input_deconvolutions: Partial<Record<StackColorRole, StackDeconvolutionConfig>>;
  input_stretches: Partial<Record<StackColorRole, StackStretchRequest[]>>;
  output_stretches: StackStretchRequest[];
}

export interface StackBackgroundConfig {
  model: { kind: 'polynomial'; degree: number; ridge: number };
  samples_per_axis: number;
  sample_radius: number | null;
  search_steps: number;
  sample_rejection_sigma: number;
  fit_rejection_sigma: number;
  fit_rejection_iterations: number;
  border_fraction: number;
}

export interface StackBackgroundExtraction {
  config: StackBackgroundConfig;
  correction_mode: 'subtract' | 'divide';
}

export interface StackBackgroundFit {
  width: number;
  height: number;
  channels: number;
  model: { kind: 'polynomial'; degree: number; coefficients: number[][] };
  reference: number[];
  samples: Array<{
    x: number;
    y: number;
    values: number[];
    dispersion: number;
    weight: number;
    status: 'accepted' | 'rejected_noise' | 'rejected_residual';
  }>;
  diagnostics: {
    candidate_samples: number;
    accepted_samples: number;
    rejected_noise: number;
    rejected_residual: number;
    rejection_iterations: number;
    sample_radius: number;
  };
}

export type StackColorProgressState =
  | 'pending'
  | 'running'
  | 'completed'
  | 'skipped'
  | 'reused'
  | 'failed';

export type StackColorProgressPhase =
  | 'loading_sources'
  | 'background_preparation'
  | 'registering_sources'
  | 'deconvolving_inputs'
  | 'normalizing_inputs'
  | 'stretching_inputs'
  | 'composing_color'
  | 'stretching_output'
  | 'writing_fits'
  | 'rendering_original'
  | 'rendering_screen'
  | 'publishing_artifacts';

export interface StackColorPhaseProgress {
  phase: StackColorProgressPhase;
  label: string;
  state: StackColorProgressState;
  completed_units: number;
  total_units: number;
}

export interface StackColorProgress {
  completed_units: number;
  total_units: number;
  active_phase: StackColorProgressPhase | null;
  current_role: StackColorRole | null;
  current_stage: number | null;
  stage_count: number | null;
  phases: StackColorPhaseProgress[];
}

export interface StackColorJob {
  schema_version: number;
  job_id: string;
  database_id: string;
  project_id: number;
  target_id: number;
  target_name: string;
  kind: StackColorKind;
  palette: StackNarrowbandPalette | null;
  label: string;
  state: StackJobState;
  phase: string;
  processed_channels: number;
  total_channels: number;
  progress: StackColorProgress;
  created_unix_seconds: number;
  artifact_revision: string;
  cache_version: number;
  stacking_version: string;
  background_version: string;
  deconvolution_version: string;
  linear_input_id: string | null;
  sources: StackColorSource[];
  processing: StackColorProcessing | null;
  resolved_input_stretches: Partial<Record<StackColorRole, unknown[]>>;
  resolved_input_deconvolutions: Partial<Record<StackColorRole, StackDeconvolutionResult>>;
  resolved_output_stretches: unknown[];
  resolved_backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  preview_url: string;
  fits_url: string;
  error: string | null;
  outdated: boolean;
  outdated_reason: string | null;
}

export interface StackColorAvailableRole {
  role: StackColorRole;
  filter_name: string;
}

export interface StackColorTargetAvailability {
  target_id: number;
  target_name: string;
  available_roles: StackColorAvailableRole[];
  ambiguous_roles: StackColorRole[];
  unmapped_filters: string[];
  rgb_available: boolean;
  lrgb_available: boolean;
  narrowband_palettes: StackNarrowbandPalette[];
}

export interface StackColorCatalog {
  schema_version: number;
  database_id: string;
  project_id: number;
  targets: StackColorTargetAvailability[];
  jobs: StackColorJob[];
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

/** Per-project line of an import outcome report. */
export interface ImportProjectSummary {
  name: string;
  targets: number;
  frames: number;
}

/** One existing target that received attached frames. */
export interface ImportAttachSummary {
  project: string;
  target: string;
  frames: number;
  /** 'name' or 'coordinates' */
  matched_by: string;
}

/** Result of one FITS import run (mirrors Rust `ImportOutcome`). */
export interface ImportOutcome {
  scanned: number;
  unreadable: number;
  non_light: number;
  skipped_existing: number;
  imported: number;
  /** Frames attached to targets that already existed. */
  attached: number;
  projects_created: number;
  targets_created: number;
  templates_created: number;
  templates_reused: number;
  plans_created: number;
  profile_id: string;
  dry_run: boolean;
  project_summaries: ImportProjectSummary[];
  attach_summaries: ImportAttachSummary[];
  created_target_ids: number[];
  attached_target_ids: number[];
}

/** Progress of the singleton per-DB import job (poll ~1s while running). */
export interface ImportJobProgress {
  running: boolean;
  /** scanning | importing | backfill | complete | error | "" (never ran) */
  stage: string;
  image_dirs: string[];
  total_files: number;
  scanned_files: number;
  outcome?: ImportOutcome | null;
  backfill_total: number;
  backfill_done: number;
  backfill_current_target?: number | null;
  started_at?: number | null;
  finished_at?: number | null;
  error?: string | null;
}

export interface ImportStatus {
  /** POST: whether this request started a job. GET: whether one is running. */
  started: boolean;
  progress: ImportJobProgress;
}

/** Body of `POST /api/db/{db_id}/import`. */
export interface ImportRequest {
  image_dirs?: string[];
  time_gap_days?: number;
  profile_id?: string;
  dry_run?: boolean;
  backfill?: boolean;
  /** Attach to existing targets by name/coordinates (default true). */
  attach_existing?: boolean;
  match_radius_deg?: number;
}

/** Body of `POST /api/databases/create`. */
export interface CreateDatabaseRequest {
  name: string;
  image_dirs: string[];
  db_path?: string;
  slug?: string;
  time_gap_days?: number;
  profile_id?: string;
  backfill?: boolean;
}

export interface CreateDatabaseResponse {
  database: DatabaseSummary;
  import: ImportJobProgress;
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
  satellite?: {
    predicted_tracks: number;
    potentially_bright_count: number;
    high_risk_count: number;
    maximum_bright_trail_risk: number;
    pixel_alignment_attempted: boolean;
    pixel_aligned_count: number;
    pixel_aligned_high_risk_count: number;
    reject_recommended: boolean;
    association: 'predicted_not_pixel_detected' | 'predicted_pixel_checked' | 'predicted_with_pixel_alignment';
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
  force_satellites?: boolean;
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
  satellite_risk_count: number;
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
