/** What a backup is doing, in words, for Settings and the job line. */
import type { BackupJob } from '../api/backupTypes';

const STAGES: Record<string, string> = {
  preparing: 'Preparing',
  copying: 'Copying',
  compressing: 'Compressing',
  finishing: 'Finishing',
  pruning: 'Removing older backups',
};

/** `Copying catalog imaging-rig · 2/9 files`, or what it waits for. */
export function describeJob(job: BackupJob): string {
  if (job.stopping) return 'Stopping…';
  if (job.waiting_for) return `Waiting for ${job.waiting_for}`;
  const stage = STAGES[job.stage] ?? job.stage;
  const what = job.current ? `${stage} ${job.current}` : stage;
  return job.files_total > 0 ? `${what} · ${job.files_done}/${job.files_total} files` : what;
}

export function jobPercent(job: BackupJob): number | null {
  return job.bytes_total > 0 ? Math.min(100, (job.bytes_done / job.bytes_total) * 100) : null;
}
