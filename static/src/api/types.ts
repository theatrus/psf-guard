import type { OverlaySolution } from '@seiza/astro-overlay';

export interface Project {
  id: number;
  profile_id: string;
  profile_name: string;
  name: string;
  display_name: string;
  description: string | null;
  has_files: boolean;
  state: number;
  latest_image_date: number | null;
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

export interface TargetNavigation {
  id: number;
  project_id: number;
  name: string;
  active: boolean;
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
  /** PSF orientation in radians (elongation direction), when fitted. */
  theta?: number | null;
}

export interface StarDetectionResponse {
  detected_stars: number;
  average_hfr: number;
  average_fwhm: number;
  /** Frame dimensions in pixels; absent from pre-v3 cached results. */
  width?: number | null;
  height?: number | null;
  stars: StarInfo[];
  /** Per-region statistics over the 3×3 grid, computed server-side by
   * seiza-stars. Absent only from a server predating the field. */
  cells?: TiltCell[];
  tilt?: TiltSummaryInfo | null;
}

/** One 3×3 grid cell's aggregate star statistics (server-computed). */
export interface TiltCell {
  row: number;
  col: number;
  star_count: number;
  median_hfr: number | null;
  median_eccentricity: number | null;
  /** Mean elongation direction in radians over [0, π); axial circular mean. */
  mean_theta: number | null;
  /** Direction agreement, 0 (random) to 1 (aligned). */
  theta_coherence: number;
}

export type TiltCornerName = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

export interface TiltCorner {
  corner: TiltCornerName;
  hfr: number | null;
}

/** ASTAP-style corner-vs-center tilt and curvature verdict (server-computed). */
export interface TiltSummaryInfo {
  center_hfr: number | null;
  corners: TiltCorner[];
  mean_hfr: number | null;
  tilt_percent: number | null;
  curvature_percent: number | null;
  worst_corner: TiltCornerName | null;
  best_corner: TiltCornerName | null;
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

/** One grade assignment inside a batch; entries may carry different
 * statuses so an undo can restore a mixed selection in one request. */
export interface BatchGradeEntry {
  image_id: number;
  status: 'pending' | 'accepted' | 'rejected';
  reason?: string | null;
}

export interface BatchGradeResponse {
  updated: number;
  /** The grades the batch replaced, for undo state. */
  previous: BatchGradeEntry[];
}

export interface PreviewOptions {
  size?: 'screen' | 'large' | 'original';
  stretch?: boolean;
  midtone?: number;
  shadow?: number;
  max_stars?: number;
  /** Render a one-shot-color mosaic in colour. Mono frames ignore it. */
  color?: boolean;
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
  /**
   * Must match the flag the `<img>` requested. The server keys colour and
   * greyscale renditions separately, so polling without it would report on
   * the wrong artifact.
   */
  color?: boolean;
}

export type StackJobState = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
export type StackGroupState =
  | 'queued'
  | 'running'
  | 'ready'
  | 'skipped'
  | 'error'
  | 'cancelled';

export interface StackFrameDecision {
  image_id: number;
  disposition: 'excluded' | 'reference' | 'accepted' | 'rejected';
  reason: string | null;
  quality_score: number | null;
  matched_stars: number | null;
  registration_rms_pixels: number | null;
  registration_drift_pixels: number | null;
  registered_mapping: unknown | null;
  normalization_mean_gain: number | null;
  normalization_mean_offset: number | null;
  source_fingerprint: string | null;
  overlap_fraction: number | null;
  integrated_fraction: number | null;
}

export interface SimilarityTransform {
  scale: number;
  rotation_radians: number;
  translation_x: number;
  translation_y: number;
}

export interface ReferenceRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type ArtifactSearchState = 'queued' | 'running' | 'completed' | 'failed';

export interface ArtifactSearchResult {
  image_id: number;
  filter_name: string;
  acquired_unix_seconds: number | null;
  grading_status: number;
  score: number;
  peak_sigma: number;
  bright_fraction: number;
  dark_fraction: number;
  coverage_fraction: number;
  evidence: 'strong' | 'possible' | 'low';
  direction: 'bright' | 'dark' | 'mixed';
  morphology: 'ring' | 'broad_dark' | 'linear' | 'compact' | 'diffuse' | 'unclassified';
  crop_url: string;
}

export interface ArtifactSearchJob {
  schema_version: number;
  search_id: string;
  database_id: string;
  source_job_id: string;
  source_kind: 'mono' | 'color';
  group_index: number | null;
  artifact_revision: string;
  region: ReferenceRegion;
  state: ArtifactSearchState;
  phase: string;
  total_work_units: number;
  completed_work_units: number;
  created_unix_seconds: number;
  notes: string[];
  results: ArtifactSearchResult[];
  error: string | null;
}

export interface StackInputImage {
  image_id: number;
  grading_status: number;
}

/**
 * How the published stack pixels are laid out. `source_frame` keeps the
 * reference frame's own rotation and is the default; `north_up_east_left`
 * means the build was reprojected onto the shared celestial grid. A
 * `source_frame` stack keeps its reference frame's rotation, turned half a
 * turn where the reference sits on the far side of a meridian flip.
 */
/**
 * How an export arranges its files.
 *
 * `standard` groups by target, PSF Guard's own tree. `wbpp` gives each frame
 * type its own root for WeightedBatchPreprocessing, and folds dark flats in
 * with the darks because WBPP has no dark-flat type of its own.
 */
export type ExportLayout = 'standard' | 'wbpp';

export interface StackSkyOrientation {
  convention: 'north_up_east_left' | 'source_frame';
  version: number;
  /**
   * What decided the layout. A `north_up_east_left` stack names the solve it
   * was reprojected from. A `source_frame` stack names the anchor that chose
   * which way up it was published: the reference frame's own solved sky
   * rotation, its side of the pier, or — with neither available — which way
   * most of that stack's own exposure faced.
   */
  source:
    | 'cached_pixel_solve'
    | 'embedded_wcs'
    | 'stack_reference_solve'
    | 'sky_anchor'
    | 'pier_side'
    | 'exposure_majority';
  output_width: number;
  output_height: number;
  source_to_output: unknown;
}

/**
 * The order a build integrated its frames in, which decides what its
 * progressive signal-to-noise curve answers. `capture` starts with the best
 * registration reference, then follows the remaining frames chronologically;
 * `quality` puts the best-graded frames first and asks which are worth keeping.
 */
export type StackFrameOrder = 'capture' | 'quality';

/** One depth on a progressive signal-to-noise curve. */
export interface SnrPoint {
  frames: number;
  exposure_seconds: number;
  /** Pixel-to-pixel noise of the integration, in the stack's own units. */
  noise: number;
  background: number;
  signal: number;
  snr: number;
  channel_noise?: number[];
}

/** Where the curve stands, in one word. */
export type SnrVerdict = 'uncertain' | 'improving' | 'diminishing' | 'plateau' | 'degrading';

/** What the fitted trend says more frames would buy. A prediction. */
export interface SnrProjection {
  /** The gain being priced, as a multiplier: 1.05 is five percent. */
  gain: number;
  extra_frames: number;
  extra_seconds: number;
}

/** A span of the curve where the noise rose instead of falling. */
export interface SnrRegression {
  from_frames: number;
  to_frames: number;
  noise_increase: number;
}

export interface SnrAnalysis {
  measured_frames: number;
  measured_seconds: number;
  best_snr: number;
  final_noise: number;
  /** The exponent in `noise ∝ frames^b`, fitted over the deeper points. */
  noise_exponent: number;
  overall_noise_exponent: number;
  fit_r_squared: number;
  /** What perfect averaging gives: −0.5. */
  ideal_exponent: number;
  /** `noise_exponent` against the ideal, so 1 is textbook square-root gain. */
  efficiency: number;
  frames_for_90_percent: number | null;
  seconds_for_90_percent: number | null;
  frames_for_95_percent: number | null;
  seconds_for_95_percent: number | null;
  projections: SnrProjection[];
  regressions: SnrRegression[];
  verdict: SnrVerdict;
  summary: string;
}

export interface ProgressiveSnr {
  order: StackFrameOrder;
  points: SnrPoint[];
  /** Absent until three depths have been measured. */
  analysis: SnrAnalysis | null;
  /** Why the measured curve cannot support a fitted trend, when applicable. */
  analysis_reason?: string | null;
}

export interface StackGroupStatus {
  index: number;
  target_id: number;
  target_name: string;
  filter_name: string;
  state: StackGroupState;
  phase: string;
  total_candidates: number;
  eligible_frames: number;
  quality_excluded: number;
  missing_files: number;
  processed_frames: number;
  /** Frames restored from a saved accumulator; zero for a fresh build. */
  reused_frames?: number;
  /** Why an existing checkpoint could not be extended, when that happened. */
  resume_note?: string | null;
  accepted_frames: number;
  rejected_frames: number;
  output_channels: number;
  sky_orientation: StackSkyOrientation | null;
  reference_image_id: number | null;
  total_exposure_seconds: number;
  preview_url: string | null;
  fits_url: string | null;
  /** The curve this build measured, and what it reads as. */
  snr?: ProgressiveSnr | null;
  /** Where the curve is published as JSON, beside the stack's own FITS. */
  snr_url?: string | null;
  error: string | null;
  calibration: AppliedCalibration;
  input_images: StackInputImage[];
  frames: StackFrameDecision[];
}

/**
 * How a stack build calibrates its lights. `auto` applies the safe masters
 * and refuses combinations that damage the result; `on` forces every
 * buildable master, including the refused combinations; `off` stacks raw
 * frames.
 */
export type CalibrationMode = 'auto' | 'on' | 'off';

export interface AppliedCalibration {
  /** Absent on artifacts recorded before the mode existed; those ran `auto`. */
  mode?: CalibrationMode;
  state: 'none' | 'matching' | 'incomplete' | 'applied' | 'off';
  bias_frames: number;
  dark_frames: number;
  dark_flat_frames: number;
  flat_frames: number;
  bias_master: string | null;
  dark_master: string | null;
  dark_flat_master: string | null;
  flat_master: string | null;
  warning: string | null;
  fingerprint: string;
  /**
   * The pedestal subtracted before flat division when the library holds no
   * bias or dark master, fitted from the lights. Absent for stacks that
   * calibrated with measured masters.
   */
  estimated_pedestal_adu?: number | null;
  /**
   * How many calibration sessions the group's lights partitioned into. A
   * multi-night stack calibrates each session with its own masters. Absent
   * or zero on artifacts recorded before sessions existed.
   */
  sessions?: number;
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
  /** The order every group integrated its frames in. */
  order?: StackFrameOrder;
  groups: StackGroupStatus[];
  error: string | null;
}

export interface LatestStackPreviewGroup {
  job_id: string;
  artifact_revision: string;
  accepted_only: boolean;
  created_unix_seconds: number;
  cache_version: number;
  /** The order this artifact integrated its frames in; old artifacts used capture order. */
  order?: StackFrameOrder;
  group: StackGroupStatus;
}

export interface LatestStackPreviews {
  schema_version: number;
  database_id: string;
  project_id: number;
  updated_unix_seconds: number;
  groups: LatestStackPreviewGroup[];
}

export type StackActivityKind = 'mono' | 'color';

/** One queued or running stack build, reported across databases. */
export interface StackActivityEntry {
  kind: StackActivityKind;
  job_id: string;
  database_id: string;
  project_id: number;
  state: StackJobState;
  label: string;
  detail: string;
  processed_units: number;
  total_units: number;
  created_unix_seconds: number;
}

export interface StackActivity {
  schema_version: number;
  active: StackActivityEntry[];
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
  rc_astro?: RcAstroProcessing | null;
}

/** One RC-Astro schema parameter's type, default, and range. */
export type ExternalParameterKind =
  | { type: 'float'; default: number; min: number; max: number }
  | { type: 'bool'; default: boolean }
  | { type: 'int'; default: number; min: number; max: number };

export interface ExternalToolParameter {
  name: string;
  /** The CLI flag; null marks a GUI-only parameter that cannot be set. */
  flag: string | null;
  label: string;
  description: string;
  kind: ExternalParameterKind;
}

/** One RC-Astro tool's live contract, read from `rc-astro <tool> --json`. */
export interface ExternalToolSchema {
  schema_version: number;
  cli_version: string;
  key: string;
  name: string;
  ml_version: number | null;
  licensed: boolean;
  license_message: string | null;
  parameters: ExternalToolParameter[];
}

export interface RcAstroCapabilities {
  available: boolean;
  executable?: string;
  tools: ExternalToolSchema[];
  error?: string;
}

export type RcAstroParameterValue = number | boolean;

export interface RcAstroStep {
  tool: string;
  parameters: Record<string, RcAstroParameterValue>;
}

export interface RcAstroProcessing {
  steps: RcAstroStep[];
}

export interface RcAstroStepResult {
  tool: string;
  name: string;
  ml_version: number | null;
  device?: string;
  warnings: string[];
}

export interface StackRcAstroResult {
  cli_version: string;
  steps: RcAstroStepResult[];
  has_stars: boolean;
}

/** A 202 poll answer while a detached processing run computes. */
export interface StackStretchPendingProgress {
  pending: true;
  /** The tool currently running, e.g. "RC-Astro StarXTerminator". */
  stage?: string;
  /** The chain's overall fraction complete in 0..1. */
  fraction?: number;
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
  rc_astro?: StackRcAstroResult | null;
  rc_astro_id?: string | null;
  preview_url: string;
  original_preview_url: string;
  fits_url: string | null;
  /** Present when star removal kept a stars image. */
  stars_preview_url?: string | null;
  stars_original_preview_url?: string | null;
  stars_fits_url?: string | null;
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
  reference_image_id: number | null;
  sky_orientation: StackSkyOrientation | null;
  registration_transform: SimilarityTransform | null;
}

export interface StackColorProcessing {
  background_extraction: StackBackgroundExtraction | null;
  input_deconvolutions: Partial<Record<StackColorRole, StackDeconvolutionConfig>>;
  input_stretches: Partial<Record<StackColorRole, StackStretchRequest[]>>;
  output_stretches: StackStretchRequest[];
}

/**
 * A named, database-independent capture of one processing editor's
 * parameters. `view` setups hold a StackViewProcessingRequest; `color` setups
 * hold a StackColorProcessing. The server stores the canonical form and
 * validates the shape on save and import.
 */
export type ProcessingSetupKind = 'view' | 'color';

export interface ProcessingSetup {
  name: string;
  kind: ProcessingSetupKind;
  settings: unknown;
  created_unix_seconds: number;
  updated_unix_seconds: number;
}

/** Also the export document: save it verbatim and the import accepts it. */
export interface ProcessingSetupsDocument {
  schema_version: number;
  setups: ProcessingSetup[];
}

export interface ProcessingSetupsImportResult {
  imported: number;
  replaced: number;
  setups: ProcessingSetup[];
}

export interface StackBackgroundConfig {
  model:
    | {
        kind: 'automatic';
        max_degree: number;
        ridge: number;
        rbf_smoothing: number;
        max_control_points: number;
        allow_radial_basis: boolean;
        minimum_improvement: number;
      }
    | { kind: 'polynomial'; degree: number; ridge: number }
    | { kind: 'radial_basis'; smoothing: number; max_control_points: number };
  samples_per_axis: number;
  sample_radius: number | null;
  search_steps: number;
  sample_rejection_sigma: number;
  fit_rejection_sigma: number;
  fit_rejection_iterations: number;
  border_fraction: number;
  /** The server derives these from fresh pixel solves. */
  protected_regions?: never[];
}

export interface StackBackgroundExtraction {
  config: StackBackgroundConfig;
  correction_mode: 'subtract' | 'divide';
  strength: number;
  protect_catalog_emission: boolean;
}

export interface StackBackgroundFit {
  width: number;
  height: number;
  channels: number;
  model:
    | { kind: 'polynomial'; degree: number; coefficients: number[][] }
    | {
        kind: 'radial_basis';
        smoothing: number;
        centers: number[][];
        coefficients: number[][];
      };
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
    protected_regions: number;
    model_selection?: {
      selected: string;
      candidates: Array<{ model: string; validation_error: number }>;
    };
  };
}

export interface StackBackgroundProtection {
  reference_image_id: number;
  catalog_version: string | null;
  object_names: string[];
  region_count: number;
  region_fingerprint: string;
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

export type StackColorCrop = 'none' | 'bounds' | 'inscribed';

export interface StackColorChannelCoverage {
  role: StackColorRole | null;
  name: string;
  covered_pixels: number;
  center_offset_pixels: number;
  off_center: boolean;
}

export interface StackColorCropReport {
  grid_width: number;
  grid_height: number;
  x: number;
  y: number;
  width: number;
  height: number;
  retained_fraction: number;
  channels: StackColorChannelCoverage[];
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
  crop: StackColorCrop;
  crop_report: StackColorCropReport | null;
  processing: StackColorProcessing | null;
  resolved_input_stretches: Partial<Record<StackColorRole, unknown[]>>;
  resolved_input_deconvolutions: Partial<Record<StackColorRole, StackDeconvolutionResult>>;
  resolved_output_stretches: unknown[];
  resolved_backgrounds: Partial<Record<StackColorRole, StackBackgroundFit>>;
  resolved_background_protection: Partial<Record<StackColorRole, StackBackgroundProtection>>;
  background_protection_fallbacks: Partial<Record<StackColorRole, string>>;
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

export interface SiteBanner {
  title: string;
  message: string;
  link_text?: string;
  link_url?: string;
}

export type AstrometryResourceStatus =
  | 'not_configured'
  | 'missing'
  | 'available'
  | 'invalid';

export interface AstrometryResourceCapability {
  name: string;
  status: AstrometryResourceStatus;
  path?: string;
  format?: string;
  size_bytes?: number;
  modified_unix_seconds?: number;
  error?: string;
}

export interface AstrometryCapabilities {
  seiza_version: string;
  seiza_fits_version: string;
  data_dir?: string;
  resources: {
    objects: AstrometryResourceCapability;
    stars: AstrometryResourceCapability;
    star_identifiers: AstrometryResourceCapability;
    blind_index: AstrometryResourceCapability;
    transients: AstrometryResourceCapability;
    minor_bodies: AstrometryResourceCapability;
  };
  features: {
    object_association: boolean;
    object_name_search: boolean;
    stellar_name_search: boolean;
    hinted_solve: boolean;
    blind_solve: boolean;
    transient_annotations: boolean;
    minor_body_annotations: boolean;
  };
}

export interface AstrometryValidationReport {
  all_configured_valid: boolean;
  resources: Array<{
    name: string;
    status: AstrometryResourceStatus;
    path?: string;
    validated: boolean;
    error?: string;
  }>;
}

export type CatalogInstallPreset =
  | 'solver_lite'
  | 'solver_gaia'
  | 'blind_deep'
  | 'blind_deep_gaia20';

export interface CatalogInstallProgress {
  running: boolean;
  phase: 'idle' | 'manifest' | 'downloading' | 'installing' | 'complete' | 'error';
  message: string;
  preset?: CatalogInstallPreset;
  output_dir?: string;
  file_name?: string;
  files_completed: number;
  files_total: number;
  bytes_completed?: number;
  bytes_total?: number;
  written_bytes?: number;
  installed_version?: string;
  error?: string;
  started_at?: number;
  finished_at?: number;
}

export interface CatalogInstallStatus {
  started: boolean;
  progress: CatalogInstallProgress;
}

export interface CalibrationSettings {
  /** Configured override in degrees; null when the library default applies. */
  rotation_tolerance_deg: number | null;
  /** What applies with no override, for labeling the placeholder. */
  default_rotation_tolerance_deg: number;
}

export interface ExportSettings {
  /** The layout the export dialog starts from. */
  default_layout: ExportLayout;
}

export interface ServerInfo {
  version: string;
  cache_directory: string;
  /** Whether database mutations and sync are accepted on this server. */
  allow_database_management: boolean;
  /** Optional plain-text notice configured by the server administrator. */
  banner?: SiteBanner;
}

export type AccessRole = 'read_only' | 'read_write';

export interface AuthStatus {
  authentication_required: boolean;
  authenticated: boolean;
  role?: AccessRole;
  username?: string;
  can_compute: boolean;
}

export interface AuthUserSummary {
  username: string;
  role: AccessRole;
  email?: string;
}

export interface CreateAuthUserRequest {
  username: string;
  role: AccessRole;
  email?: string;
  password: string;
}

export interface UpdateAuthUserRequest {
  role: AccessRole;
  email?: string;
  password?: string;
}

export interface ReleaseNotice {
  schema_version: number;
  version: string;
  release_url: string;
  summary?: string;
  urgency: 'normal' | 'recommended' | 'required';
  minimum_supported_version?: string;
  published_at?: string;
}

export interface UpdateNoticeStatus {
  notice?: ReleaseNotice;
  checking: boolean;
  checked_at_unix_seconds?: number;
}

/** One configured database, returned by /api/databases. */
export interface DatabaseSummary {
  id: string;
  name: string;
  database_path: string;
  image_directories: string[];
  remote_image_upload: RemoteImageUploadSummary;
  /**
   * Operator-configured server-side export destination. When present the
   * UI offers a server export that runs without database management.
   */
  export_directory?: string;
}

/** What one export placed, and how. */
export interface ExportSummary {
  planned: number;
  copied: number;
  linked: number;
  /** Copy-on-write clones: free like a hardlink, independent like a copy. */
  reflinked?: number;
  skipped_existing: number;
  missing: number;
  errors: number;
  bytes: number;
}

/** Progress of the singleton per-DB server export job. */
export interface ExportJobProgress {
  running: boolean;
  stage: string;
  destination: string;
  scope: string;
  total_files: number;
  placed_files: number;
  outcome?: ExportSummary | null;
  started_at?: number | null;
  finished_at?: number | null;
  error?: string | null;
}

export interface ExportStatus {
  started: boolean;
  progress: ExportJobProgress;
}

export interface RemoteClientSummary {
  client_uuid: string;
  name: string;
  paired_at: number;
}

export interface RemoteImageUploadSummary {
  enabled: boolean;
  image_directory?: string;
  token_configured: boolean;
  /** Remote scheduler sync is a separate grant from image upload. */
  sync_enabled: boolean;
  /** Paired clients; each holds its own revocable credential. */
  clients?: RemoteClientSummary[];
}

export type SchedulerSyncKind = 'pull' | 'push_planning' | 'push_grades';

export interface SchedulerSyncRequest {
  peer_db_id: string;
  kind: SchedulerSyncKind;
  dry_run?: boolean;
  with_image_data?: boolean;
  project?: string;
  target?: string;
  status?: 'pending' | 'accepted' | 'rejected';
  reviewed_only?: boolean;
}

export interface SchedulerSyncTableCounts {
  inserted: number;
  updated: number;
  unchanged: number;
  skipped: number;
}

export interface SchedulerSyncGradeCounts {
  source_considered: number;
  source_no_guid: number;
  matched: number;
  changed: number;
  unchanged: number;
  unmatched_source: number;
  destination_only: number;
  duplicate_guids: number;
  transitions: Record<string, number>;
}

export interface SchedulerSyncResponse {
  kind: SchedulerSyncKind;
  dry_run: boolean;
  source_db_id: string;
  destination_db_id: string;
  exposuretemplate: SchedulerSyncTableCounts;
  project: SchedulerSyncTableCounts;
  ruleweight: SchedulerSyncTableCounts;
  target: SchedulerSyncTableCounts;
  exposureplan: SchedulerSyncTableCounts;
  acquiredimage: SchedulerSyncTableCounts | null;
  imagedata: SchedulerSyncTableCounts | null;
  grades: SchedulerSyncGradeCounts | null;
  grade_filled: number;
  grade_preserved: number;
  imagedata_bytes: number;
  total_inserted: number;
  total_updated: number;
  /** Per-entity change lines, capped server-side; absent on old previews. */
  changes?: string[];
}

export interface SchedulerSyncPreviewResponse {
  preview_id: string;
  created_at: number;
  expires_at: number;
  result: SchedulerSyncResponse;
}

/** One staged transfer for a catalog, wherever it was created — the UI's
 * own preview flow or a remote client's push. */
export interface SchedulerSyncPreviewListEntry {
  preview_id: string;
  kind: string;
  source: string;
  created_at: number;
  expires_at: number;
  result: SchedulerSyncResponse;
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
  /** Masters and calibrated/registered intermediates left out. */
  skipped_processed?: number;
  /** Frames outside the run's scope. */
  skipped_out_of_scope?: number;
  calibration: CalibrationImportOutcome;
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

export interface CalibrationImportOutcome {
  imported: number;
  updated: number;
  skipped_existing: number;
  bias: number;
  dark: number;
  dark_flat: number;
  flat: number;
}

export interface CalibrationRigSummary {
  rig_uuid: string;
  name: string;
  profile_id?: string | null;
  telescope?: string | null;
  camera?: string | null;
  frame_count: number;
  bias: number;
  dark: number;
  dark_flat: number;
  flat: number;
  oldest_at?: number | null;
  newest_at?: number | null;
}

export interface CalibrationLibrarySummary {
  schema_version: number;
  frame_count: number;
  master_count: number;
  rigs: CalibrationRigSummary[];
}

export interface CalibrationFrameSummary {
  frame_uuid: string;
  rig_uuid: string;
  kind: 'bias' | 'dark' | 'dark_flat' | 'flat';
  source_path: string;
  source_exists: boolean;
  captured_at?: number | null;
  camera?: string | null;
  width?: number | null;
  height?: number | null;
  binning_x?: number | null;
  binning_y?: number | null;
  gain?: number | null;
  offset?: number | null;
  readout_mode?: number | null;
  bayer_pattern?: string | null;
  exposure_s?: number | null;
  camera_temp?: number | null;
  filter?: string | null;
  focal_length_mm?: number | null;
}

export interface CalibrationLibraryDetails {
  summary: CalibrationLibrarySummary;
  frames: CalibrationFrameSummary[];
}

export interface CalibrationMutationOutcome {
  frames_removed: number;
  masters_removed: number;
}

/** Progress of the singleton per-DB import job (poll ~1s while running). */
export interface ImportJobProgress {
  running: boolean;
  /** scanning | importing | complete | error | "" (never ran) */
  stage: string;
  image_dirs: string[];
  total_files: number;
  scanned_files: number;
  outcome?: ImportOutcome | null;
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
  /** Let the queued quality job write star count/HFR into imported images'
   *  metadata (missing keys only; default true). */
  fill_metadata?: boolean;
  /** Attach to existing targets by name/coordinates (default true). */
  attach_existing?: boolean;
  match_radius_deg?: number;
  /** Which frame kinds to import (default all). */
  scope?: ImportScope;
  /** Leave processing artifacts out of the catalog (default false). */
  skip_processed?: boolean;
}

/** How the calibration library covers one project (mirrors Rust
 *  `ProjectCalibrationReport`). */
export interface ProjectCalibrationReport {
  nights: CalibrationNightReport[];
  kinds: CalibrationKindSummary[];
  warnings: string[];
  lights_missing_files: number;
}

export interface CalibrationKindSummary {
  kind: string;
  matching_frames: number;
  sessions: string[];
  newest_at?: number | null;
  oldest_at?: number | null;
}

export interface CalibrationNightReport {
  night: string;
  lights: number;
  filters: CalibrationNightFilter[];
}

export interface CalibrationNightFilter {
  filter: string;
  lights: number;
  bias_frames: number;
  dark_frames: number;
  dark_age_days?: number | null;
  dark_flat_frames: number;
  flat_frames: number;
  flat_session?: string | null;
  flat_age_days?: number | null;
  nightly_flats: boolean;
  missing: string[];
}

/** Which frames an import run touches. */
export type ImportScope = 'all' | 'lights' | 'calibration';

/** One directory in the import folder listing (two levels deep). */
export interface ImportFolder {
  path: string;
  name: string;
  children: ImportFolder[];
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
  /** Let the queued quality job write star count/HFR into imported images'
   *  metadata (missing keys only; default true). */
  fill_metadata?: boolean;
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
  state: number;
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
  recent_images: ProjectRecentImage[];
}

export interface ProjectRecentImage {
  id: number;
  project_id: number;
  target_id: number;
  target_name: string;
  acquired_date: number | null;
  filter_name: string;
  grading_status: number;
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

export interface ExposurePlanDetails {
  id: number;
  exposure_template_id: number;
  template_name: string;
  filter_name: string;
  gain: number | null;
  offset: number | null;
  bin: number | null;
  readout_mode: number | null;
  exposure: number;
  desired: number;
  acquired: number;
  accepted: number;
  enabled: boolean;
}

export interface ExposureTemplateDetails {
  id: number;
  profile_id: string;
  name: string;
  filter_name: string;
  gain: number | null;
  offset: number | null;
  bin: number | null;
  readout_mode: number | null;
  twilight_level: number;
  moon_avoidance_enabled: boolean;
  moon_avoidance_separation: number;
  moon_avoidance_width: number;
  maximum_humidity: number;
  default_exposure: number;
  moon_relax_scale: number;
  moon_relax_max_altitude: number;
  moon_relax_min_altitude: number;
  moon_down_enabled: boolean;
  dither_every: number;
  minutes_offset: number;
  plan_count: number;
}

export interface SchedulerTargetDetails {
  id: number;
  name: string;
  active: boolean;
  ra_hours: number;
  dec_degrees: number;
  epoch_code: number;
  rotation: number;
  roi: number;
  exposure_plans: ExposurePlanDetails[];
}

export interface ProjectSchedulerDetails {
  id: number;
  profile_id: string;
  name: string;
  description: string | null;
  state: number;
  priority: number;
  created_at: number | null;
  active_at: number | null;
  inactive_at: number | null;
  minimum_time: number;
  minimum_altitude: number;
  maximum_altitude: number;
  use_custom_horizon: boolean;
  horizon_offset: number;
  meridian_window: number;
  filter_switch_frequency: number;
  dither_every: number;
  enable_grader: boolean;
  is_mosaic: boolean;
  /** Target Scheduler flats automation: 0 off, 1-7 every N sessions,
   * 100 target completion, 200 immediate. */
  flats_handling: number;
  exposure_templates: ExposureTemplateDetails[];
  targets: SchedulerTargetDetails[];
}

export interface ProjectSchedulerUpdate {
  name?: string;
  description?: string;
  state?: number;
  priority?: number;
  minimum_time?: number;
  minimum_altitude?: number;
  maximum_altitude?: number;
  use_custom_horizon?: boolean;
  horizon_offset?: number;
  meridian_window?: number;
  filter_switch_frequency?: number;
  dither_every?: number;
  enable_grader?: boolean;
  is_mosaic?: boolean;
  flats_handling?: number;
}

export interface TargetSchedulerUpdate {
  name?: string;
  project_id?: number;
  active?: boolean;
  ra_hours?: number;
  dec_degrees?: number;
  epoch_code?: number;
  rotation?: number;
  roi?: number;
}

export interface CreateExposurePlanRequest {
  exposure_template_id?: number;
  filter_name?: string;
  template_name?: string;
  gain?: number;
  offset?: number;
  bin?: number;
  readout_mode?: number;
  exposure: number;
  desired: number;
  enabled: boolean;
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

/** How hard event evidence hits the quality score: a multiplier on the
 * built-in penalty. 0 ignores the evidence, 1 (default) keeps calibrated
 * behavior, up to 2 deepens it. */
export interface PenaltyScaleParams {
  penalty_satellite?: number;
  penalty_pointing?: number;
  penalty_temporal?: number;
}

export interface SequenceAnalysisRequest extends PenaltyScaleParams {
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

export interface ProjectSequenceAnalysisRequest extends PenaltyScaleParams {
  project_id: number;
  filter_name?: string;
}

export interface DatabaseSequenceAnalysisRequest extends PenaltyScaleParams {
  all_projects: true;
  filter_name?: string;
}

export interface SequenceAnalysisResponse {
  sequences: ScoredSequence[];
  /** Scores across all stack candidates for each target/filter. Each score is
   * compared only with frames that have matching capture settings. */
  target_filter_rollups?: TargetFilterRollup[];
}

export interface TargetFilterScore {
  image_id: number;
  quality_score: number;
  normalized_metrics: ImageQualityResult['normalized_metrics'];
  details: string | null;
}

export interface TargetFilterRollup {
  target_id: number;
  target_name: string;
  filter_name: string;
  session_start?: number;
  session_end?: number;
  image_count: number;
  unavailable_image_count: number;
  images: TargetFilterScore[];
  summary: SequenceSummary;
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

export interface QualityRegionOverlay {
  grid_cols: number;
  grid_rows: number;
  image_width: number;
  image_height: number;
  low_star_cells: boolean[];
  extinction_cells: boolean[];
  star_loss_cells: boolean[];
  background_rise_cells: boolean[];
  background_fall_cells: boolean[];
  glow_cells: boolean[];
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
  /** Measured grid cells that support a localized quality finding. Global
   * findings do not offer an overlay. */
  spatial_overlay?: QualityRegionOverlay;
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
  /** Work left in a requested target/filter scope. GET only. */
  scope?: QualityScanScopeStatus;
}

export interface QualityScanScopeStatus {
  target_id: number;
  filter_name: string | null;
  total_frames: number;
  pending_frames: number;
  new_frames: number;
  outdated_frames: number;
  needs_analysis: boolean;
}

export interface SpatialScanStatusRequest {
  target_id?: number;
  filter_name?: string;
}

export interface SpatialScanRequest {
  target_id: number;
  filter_name?: string;
  force?: boolean;
  force_spatial?: boolean;
  force_astrometry?: boolean;
  force_satellites?: boolean;
  /** Write measured star count/HFR into imported images' metadata
   *  (missing keys only; default true). */
  fill_metadata?: boolean;
}

export interface QualityBackfillProgress {
  running: boolean;
  force: boolean;
  total_targets: number;
  processed_targets: number;
  current_target_id: number | null;
  started_at: number | null;
  finished_at: number | null;
}

export interface QualityBackfillStatus {
  started: boolean;
  progress: QualityBackfillProgress;
}

export interface QualityBackfillRequest {
  force?: boolean;
  /** Write measured star count/HFR into imported images' metadata
   *  (missing keys only; default true). */
  fill_metadata?: boolean;
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

/** A remote PSF Guard this instance can sync with. The key never leaves the server. */
export interface PeerSummary {
  id: string;
  name: string;
  base_url: string;
  catalog_id: string | null;
  token_configured: boolean;
}

export interface PeerCheck {
  reachable: boolean;
  product: string | null;
  product_version: string | null;
  protocol_version: number | null;
  catalogs: string[];
  capabilities: string[];
  error: string | null;
}

export type RemoteSyncDirection = 'pull' | 'push_planning' | 'push_grades';

export interface RemoteSyncRequest {
  peer_id: string;
  direction: RemoteSyncDirection;
  dry_run: boolean;
  reviewed_only?: boolean;
  with_image_data?: boolean;
}

export interface RemoteSyncResult {
  applied: boolean;
  peer_product: string;
  peer_catalog: string;
  summary: Record<string, number>;
}
