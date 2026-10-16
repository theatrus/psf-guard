/** Settings › Backups and the backup job. Mirrors `src/server/backup_job.rs`. */

export type BackupScope = 'meta' | 'full';
export type BackupTrigger = 'manual' | 'schedule' | 'cli';
export type BackupLocation = 'destination' | 'downloads' | 'elsewhere';

export type BackupRetention =
  | { kind: 'count'; count: number }
  | { kind: 'age'; days: number };

/** The periodic backup, as the registry keeps it. */
export interface BackupSettings {
  enabled: boolean;
  frequency: 'daily' | 'weekly';
  /** 0 is Monday, 6 is Sunday. */
  weekday: number;
  /** Local time of day on the server, `HH:MM`. */
  time: string;
  destination?: string | null;
  scope: BackupScope;
  include_secrets: boolean;
  retention: BackupRetention;
  since?: number | null;
}

/** The run under way. */
export interface BackupJob {
  trigger: BackupTrigger;
  scope: BackupScope;
  include_secrets: boolean;
  destination: string;
  location: BackupLocation;
  stage: 'preparing' | 'copying' | 'compressing' | 'finishing' | 'pruning' | string;
  current: string | null;
  files_done: number;
  files_total: number;
  bytes_done: number;
  bytes_total: number;
  waiting_for: string | null;
  started_at: number;
  stopping: boolean;
}

export interface BackupLastRun {
  trigger: BackupTrigger;
  scope: BackupScope;
  started_at: number;
  finished_at: number;
  outcome: 'succeeded' | 'failed' | 'stopped';
  location: BackupLocation;
  destination: string;
  file_name: string | null;
  bytes: number | null;
  error: string | null;
  problems: string[];
  pruned: string[];
}

export interface BackupListing {
  file_name: string;
  bytes: number;
  created_at: string;
  created_unix: number;
  scope: BackupScope;
  trigger: BackupTrigger;
  secrets: 'redacted' | 'included';
  psf_guard_version: string;
  catalogs: number;
  problems: number;
  location: BackupLocation;
  /** Below `/api`. */
  download_path: string;
}

export interface BackupSecretsSummary {
  peer_keys: number;
  collaboration_credentials: boolean;
  server_config_tokens: number;
}

export interface BackupsOverview {
  settings: BackupSettings;
  suggested_destination: string;
  job: BackupJob | null;
  last_run: BackupLastRun | null;
  /** Unix seconds; at or before now means due. */
  next_run_at: number | null;
  destination_warnings: string[];
  backups: BackupListing[];
  secrets: BackupSecretsSummary;
  meta_bytes: number;
  catalog_bytes: number;
  catalogs: number;
  unreachable_catalogs: string[];
}

export interface BackupJobView {
  job: BackupJob | null;
  last_run: BackupLastRun | null;
}

export interface BackupRunRequest {
  scope: BackupScope;
  include_secrets: boolean;
  /** A server folder; empty or absent makes an archive to download. */
  destination?: string | null;
}

export interface BackupDestinationCheck {
  error: string | null;
  warnings: string[];
  exists: boolean;
}

export const BACKUPS_QUERY_KEY = ['backups'] as const;
export const BACKUP_STATUS_QUERY_KEY = ['backups', 'status'] as const;
