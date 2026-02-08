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

export interface ServerInfo {
  database_path: string;
  image_directory: string;
  cache_directory: string;
  version: string;
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
  category?: string;
  normalized_metrics: {
    star_count?: number;
    hfr?: number;
    eccentricity?: number;
    snr?: number;
    background?: number;
  };
  details?: string;
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