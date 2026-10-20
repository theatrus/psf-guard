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
