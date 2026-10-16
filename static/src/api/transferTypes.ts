import type { ContributionSelection } from './collaborationTypes';
export const TRANSFER_QUERY_KEY = ['transfers'] as const;

export type TransferState = 'preparing' | 'review' | 'queued' | 'uploading' | 'paused' | 'retry' | 'needs_review' | 'complete' | 'cancelled';
export interface TransferOutput { job_id: string; group_index: number; revision: string; label: string; wbpp: boolean; matches_selection?: boolean }
export interface TransferRequest { selection: ContributionSelection; output?: Pick<TransferOutput, 'job_id' | 'group_index' | 'revision'>; confirm_wbpp_membership?: boolean }
export interface TransferManifest {
  version: number; task: string; night: string | null; panel: number;
  files: { kind: string; source: { path: string; bytes: number } }[];
  [key: string]: unknown;
}
export interface TransferJob {
  id: string; connection_id: string; revision: number; state: TransferState;
  catalog: string; label: string; total_files: number; completed_files: number;
  total_bytes: number; completed_bytes: number; current_file: string; sent_bytes: number;
  error: string | null; next_attempt_ms: number; warnings: string[] | null;
  manifest: unknown;
}
export type TransferAction = 'approve' | 'pause' | 'resume' | 'retry' | 'cancel';
export interface TransferActivity { jobs: TransferJob[]; policies?: { connection_id: string; automatic: boolean; mode: string }[] }
