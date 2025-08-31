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