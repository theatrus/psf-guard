import { useEffect, useState } from 'react';
import { Eraser } from 'lucide-react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import type { RejectRemovalPlan, RejectRemovalStatus } from '../api/types';
import Dialog from './Dialog';
import { rejectRemovalKey, rejectRemovalPollMs } from './rejectRemovalJob';
import './RejectRemovalControls.css';

const DEFAULT_DAYS = 7;
const DEFAULT_RETENTION = 14;
const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const size = (bytes: number) => bytes >= 1024 ** 3 ? `${(bytes / 1024 ** 3).toFixed(1)} GB` : `${(bytes / 1024 ** 2).toFixed(1)} MB`;
const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
const frames = (count: number) => `${count.toLocaleString()} reject${count === 1 ? '' : 's'}`;

/** **Purge Rejects** beside a database's Import: opens the removal flow in
 *  a dialog. Only with database management. */
export function PurgeRejectsButton({ dbId, dbName, canManage, disabled }: { dbId: string; dbName: string; canManage: boolean; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  if (!canManage) return null;
  return <>
    <button type="button" className="db-action-button is-icon" disabled={disabled} onClick={() => setOpen(true)}
      aria-label={`Purge rejects in ${dbName}`}
      title="Purge rejects: remove older rejected frames from the catalog and move their files to trash">
      <Eraser size={15} aria-hidden="true" />
    </button>
    <Dialog open={open} onClose={() => setOpen(false)} title={`Purge rejects — ${dbName}`} className="purge-rejects-dialog">
      <RejectRemovalControls dbId={dbId} canManage={canManage} />
    </Dialog>
  </>;
}

/** What a finished run of this page's own did, in words. */
function outcomeNotice(status: RejectRemovalStatus): string {
  if (status.state === 'failed') return '';
  if (status.changed) return 'The rejected frames changed since the preview. Review the updated preview.';
  const outcome = status.outcome?.run === status.run ? status.outcome : null;
  if (status.state === 'stopped' && !outcome) return 'Stopped; nothing changed.';
  if (outcome?.removed) {
    const report = outcome.removed;
    return `Removed ${frames(report.removed.length)}; files remain in the trash until ${day(report.trash_until)}.`
      + (report.stopped ? ' Stopped. The remaining frames stay in the catalog.' : '')
      + (report.failed.length ? ` ${frames(report.failed.length)} could not be removed: ${report.failed[0].reason}` : '');
  }
  if (outcome?.restored) {
    const report = outcome.restored;
    return `Restored ${frames(report.restored.length)}.`
      + (report.stopped ? ' Stopped. The remaining frames have not been restored.' : '')
      + (report.failed.length ? ` ${frames(report.failed.length)} could not be restored: ${report.failed[0].reason}` : '');
  }
  if (outcome?.emptied) {
    const report = outcome.emptied;
    return `Deleted ${report.files_deleted.toLocaleString()} file${report.files_deleted === 1 ? '' : 's'} (${size(report.bytes)}) from the trash.`
      + (report.stopped ? ' Stopped. Choose Empty trash again to delete the remaining files.' : '')
      + (report.problems.length ? ` ${report.problems[0]}` : '');
  }
  return '';
}

/** Remove rejects that have stayed rejected a while: their files go to a
 *  trash folder and can come back until the trash is emptied. See
 *  docs/design/reject-removal.md. Only with database management.
 *
 *  Preview, Remove, Restore and Empty trash read the image folders and move
 *  files, so each runs as the catalog's reject removal job: this page starts
 *  it, follows it, and can stop it, and the header's job line lists it. */
export default function RejectRemovalControls({ dbId, canManage }: { dbId: string; canManage: boolean }) {
  const client = useQueryClient();
  const removedKey = ['db', dbId, 'removed-rejects'] as const;
  const statusKey = rejectRemovalKey(dbId);
  const removed = useQuery({ queryKey: removedKey, queryFn: () => apiClient.getRemovedRejects(dbId), enabled: canManage, staleTime: 60_000, retry: false });
  const job = useQuery({
    queryKey: statusKey,
    queryFn: () => apiClient.getRejectRemoval(dbId),
    enabled: canManage,
    retry: false,
    refetchInterval: (query) => rejectRemovalPollMs(query.state.data),
  });
  const status = job.data;
  const running = status?.state === 'running';
  const [days, setDays] = useState(DEFAULT_DAYS);
  const [retention, setRetention] = useState(DEFAULT_RETENTION);
  /** The run this page started, whose end it reports. */
  const [mine, setMine] = useState<number | null>(null);
  const [notice, setNotice] = useState('');
  const refresh = () => {
    void client.invalidateQueries({ queryKey: removedKey });
    void client.invalidateQueries({ queryKey: ['db', dbId], predicate: query => query.queryKey[2] !== 'rejects' });
  };
  const start = useMutation({
    retry: false,
    mutationFn: (begin: () => Promise<RejectRemovalStatus>) => begin(),
    onSuccess: next => {
      client.setQueryData(statusKey, next);
      setMine(next.run);
      setNotice('');
    },
  });
  const stop = useMutation({
    retry: false,
    mutationFn: () => apiClient.stopRejectRemoval(dbId),
    onSuccess: next => client.setQueryData(statusKey, next),
  });
  const purge = useMutation({
    retry: false,
    mutationFn: (batch: string) => apiClient.purgeRemovedRejects(dbId, batch),
    onSuccess: report => { setNotice(`Purged the saved records of ${frames(report.frames)}; they can no longer be restored.`); refresh(); },
  });
  // A run that ended changed the catalog's lists: read them again, and say
  // what this page's own run did.
  const finished = status && status.state !== 'running' ? status.finished_at : null;
  useEffect(() => {
    if (!finished || !status) return;
    if (status.action !== 'preview') refresh();
    if (status.run === mine) setNotice(outcomeNotice(status));
    // Only a new end matters; the lists and notice follow it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [finished, status?.run]);
  if (!canManage) return null;
  const now = Date.now() / 1000;
  const batches = removed.data?.batches ?? [];
  const emptiable = batches.some(batch => batch.files_deleted < batch.frames && batch.trash_until <= now);
  // The preview shows for the age it was made with, until a removal spends it.
  const plan: RejectRemovalPlan | null = !running && status?.plan && status.plan.min_age_days === days ? status.plan : null;
  // Why rejects stay, counted: a shared file names the other frame, so those group as one.
  const reasons = plan ? Object.entries(plan.skipped.reduce<Record<string, number>>((counts, skip) => {
    const reason = skip.reason.startsWith('its file also backs') ? 'their file backs another frame' : skip.reason;
    counts[reason] = (counts[reason] ?? 0) + 1;
    return counts;
  }, {})) : [];
  const busy = running || start.isPending || purge.isPending;
  const runningAs = (action: RejectRemovalStatus['action']) => running && status?.action === action;
  const failure = status?.state === 'failed' && status.run === mine ? status.error : null;
  const error = start.error ?? stop.error ?? purge.error;
  return (
    <div className="purge-rejects" role="group" aria-label="Remove rejects">
      <small>Removed frames leave the catalog; their files wait in a trash folder, and can be restored, until the trash is emptied.</small>
      <div className="reject-removal-row">
        <label>
          Rejects rejected
          <input type="number" min={0} max={3650} step={1} aria-label="Days rejected" value={days} disabled={busy}
            onChange={event => setDays(Math.max(0, Math.round(Number(event.target.value) || 0)))} />
          days or more
        </label>
        <button type="button" className="browse-button" disabled={busy}
          onClick={() => start.mutate(() => apiClient.previewRejectRemoval(dbId, days))}>
          {runningAs('preview') ? 'Looking…' : 'Preview'}
        </button>
      </div>
      {running && status && <div className="reject-removal-progress" role="status" aria-label="Reject removal progress">
        <span>{status.title} · {status.label}</span>
        <button type="button" className="browse-button" disabled={stop.isPending} onClick={() => stop.mutate()}
          title="Stop between frames: the frames already done stay done">
          Stop
        </button>
      </div>}
      {plan && <div className="reject-removal-plan" data-testid="reject-removal-plan">
        <small>
          {plan.frames.length > 0 ? `${frames(plan.frames.length)} to remove (${size(plan.bytes)}).` : 'Nothing to remove.'}
          {plan.waiting > 0 && ` ${frames(plan.waiting)} rejected more recently${plan.next_eligible_at ? `; the first qualifies ${day(plan.next_eligible_at)}` : ''}.`}
          {reasons.map(([reason, count]) => ` ${count} stay: ${reason}.`).join('')}
          {plan.without_files > 0 && ` ${plan.without_files} have no file on disk.`}
        </small>
        {plan.frames.length > 0 && <>
          <details>
            <summary>Frames</summary>
            <ul>{plan.frames.slice(0, 200).map(frame => <li key={frame.guid}>{frame.target_name} · {frame.file_name ?? `frame ${frame.image_id}`}{frame.reject_reason ? ` · ${frame.reject_reason}` : ''}</li>)}</ul>
            {plan.frames.length > 200 && <small>and {plan.frames.length - 200} more</small>}
          </details>
          <div className="reject-removal-row">
            <label>
              keep in the trash
              <input type="number" min={0} max={3650} step={1} aria-label="Days in the trash" value={retention} disabled={busy}
                onChange={event => setRetention(Math.max(0, Math.round(Number(event.target.value) || 0)))} />
              days
            </label>
            <button type="button" className="browse-button" disabled={busy}
              onClick={() => start.mutate(() => apiClient.applyRejectRemoval(dbId, plan.min_age_days, plan.digest, retention))}>
              {`Remove ${frames(plan.frames.length)}`}
            </button>
          </div>
        </>}
      </div>}
      {batches.length > 0 && <ul className="reject-removal-batches" aria-label="Removed rejects">
        {batches.map(batch => {
          const gone = batch.files_deleted >= batch.frames;
          const purged = batch.purged >= batch.frames;
          return <li key={batch.batch_id}>
            <span>{day(batch.removed_at)} · {frames(batch.frames)} · {size(batch.bytes)} · {purged ? 'purged' : gone ? 'files deleted' : `in the trash until ${day(batch.trash_until)}`}</span>
            {!gone && <button type="button" className="browse-button" disabled={busy} aria-label={`Restore the rejects removed ${day(batch.removed_at)}`}
              onClick={() => start.mutate(() => apiClient.restoreRemovedRejects(dbId, batch.batch_id))}>Restore</button>}
            {gone && !purged && <button type="button" className="browse-button" disabled={busy} aria-label={`Purge the rejects removed ${day(batch.removed_at)}`}
              title="Delete the saved records. PSF Guard keeps a marker to prevent reimporting these frames." onClick={() => purge.mutate(batch.batch_id)}>Purge</button>}
          </li>;
        })}
      </ul>}
      {emptiable && <button type="button" className="browse-button" disabled={busy}
        onClick={() => start.mutate(() => apiClient.emptyRejectTrash(dbId))}>
        {runningAs('empty_trash') ? 'Emptying…' : 'Empty trash'}
      </button>}
      {notice && !running && <small role="status">{notice}</small>}
      {failure && <small className="settings-error" role="alert">{failure}</small>}
      {error && <small className="settings-error" role="alert">{message(error)}</small>}
    </div>
  );
}
