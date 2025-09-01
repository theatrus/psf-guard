export interface Project {
  id: number;
  name: string;
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
  name: string;
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