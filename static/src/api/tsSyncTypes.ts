export const TS_SYNC_QUERY_KEY = ['ts-sync'] as const;

export interface TsSyncSettings {
  source_path: string;
  writeback: boolean;
  on_open: boolean;
  on_change: boolean;
  interval_minutes: number;
}

export interface TsSyncJob {
  stopping: boolean;
  waiting_for: string | null;
  id: string;
  catalog_id: string | null;
  name: string;
  running: boolean;
  automatic: boolean;
  source_connected?: boolean;
  stage: string;
  pages_done: number;
  pages_total: number;
  started_at: number;
  error: string | null;
  report: { inserted: number; updated: number; grades_written: number; conflicts: number } | null;
}
