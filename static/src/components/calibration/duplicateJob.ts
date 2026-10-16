import type { CalibrationDuplicateStatus } from '../../api/types';

/** The duplicate check or repair of one catalog: one query, shared by the
 *  Calibration page and the header's job line. */
export const duplicatesKey = (dbId: string) => ['db', dbId, 'calibrations', 'duplicates'] as const;

/** Polled while a run goes; at rest the status changes only when a person
 *  starts a run, which puts its status in the cache at once. */
export const DUPLICATES_POLL_MS = 1_000;

export function duplicatesPollMs(status: CalibrationDuplicateStatus | undefined): number | false {
  return status?.state === 'running' ? DUPLICATES_POLL_MS : false;
}

/** What each stage counts: files, copies, or exposures merged. */
const STAGE_VERBS: Record<string, string> = {
  checking_files: 'Checking files',
  comparing_copies: 'Comparing copies',
  merging: 'Merging',
};

const count = (value: number) => value.toLocaleString('en-US');

/** "Checking files 812 of 2,140", "Merging 37 of 203". */
export function duplicateJobLabel(status: Pick<CalibrationDuplicateStatus, 'stage' | 'done' | 'total'>): string {
  const verb = STAGE_VERBS[status.stage ?? ''] ?? STAGE_VERBS.checking_files;
  if (status.total <= 0) return `${verb}…`;
  return `${verb} ${count(Math.min(status.done, status.total))} of ${count(status.total)}`;
}

/** What the run is, in the job line's words. */
export function duplicateJobTitle(status: Pick<CalibrationDuplicateStatus, 'action'>): string {
  return status.action === 'repair' ? 'Repairing duplicates' : 'Checking for duplicates';
}

/** 0–100 for the run's stage, or null before it knows its size. */
export function duplicateJobPercent(status: Pick<CalibrationDuplicateStatus, 'done' | 'total'>): number | null {
  if (status.total <= 0) return null;
  return Math.min(100, (100 * status.done) / status.total);
}
