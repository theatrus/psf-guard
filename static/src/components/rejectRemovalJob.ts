import type { RejectRemovalStatus } from '../api/types';

/** One catalog's reject removal job: one query, shared by Purge Rejects and
 *  the header's job line. */
export const rejectRemovalKey = (dbId: string) => ['db', dbId, 'rejects', 'removal'] as const;

/** Polled while a run goes; at rest the status changes only when a person
 *  starts a run, which puts its status in the cache at once. */
export const REJECT_REMOVAL_POLL_MS = 1_000;

export function rejectRemovalPollMs(status: RejectRemovalStatus | undefined): number | false {
  return status?.state === 'running' ? REJECT_REMOVAL_POLL_MS : false;
}
