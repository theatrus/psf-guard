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

/** What each stage counts: image folders, frames checked, moved, restored or
 *  emptied. */
const STAGE_VERBS: Record<string, string> = {
  indexing: 'Indexing folders',
  checking: 'Checking files',
  moving: 'Moving to the trash',
  restoring: 'Restoring',
  deleting: 'Deleting',
};

const count = (value: number) => value.toLocaleString('en-US');

/** "Indexing folders 2 of 5", "Moving to the trash 37 of 203". */
export function rejectRemovalLabel(status: Pick<RejectRemovalStatus, 'stage' | 'done' | 'total'>): string {
  const verb = STAGE_VERBS[status.stage ?? ''] ?? 'Starting';
  if (status.total <= 0) return `${verb}…`;
  return `${verb} ${count(Math.min(status.done, status.total))} of ${count(status.total)}`;
}

/** What the run is, in the job line's words. */
export function rejectRemovalTitle(status: Pick<RejectRemovalStatus, 'action'>): string {
  switch (status.action) {
    case 'remove': return 'Removing rejects';
    case 'restore': return 'Restoring rejects';
    case 'empty_trash': return 'Emptying the reject trash';
    default: return 'Previewing reject removal';
  }
}

/** 0–100 for the run's stage, or null before it knows its size. */
export function rejectRemovalPercent(status: Pick<RejectRemovalStatus, 'done' | 'total'>): number | null {
  if (status.total <= 0) return null;
  return Math.min(100, (100 * status.done) / status.total);
}
