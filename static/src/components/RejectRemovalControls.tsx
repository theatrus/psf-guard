import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import type { RejectRemovalPlan } from '../api/types';

const DEFAULT_DAYS = 7;
const DEFAULT_RETENTION = 14;
const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const size = (bytes: number) => bytes >= 1024 ** 3 ? `${(bytes / 1024 ** 3).toFixed(1)} GB` : `${(bytes / 1024 ** 2).toFixed(1)} MB`;
const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
const frames = (count: number) => `${count.toLocaleString()} reject${count === 1 ? '' : 's'}`;

/** Remove rejects that have stayed rejected a while: their files go to a
 *  trash folder and can come back until the trash is emptied. See
 *  docs/design/reject-removal.md. Only with database management. */
export default function RejectRemovalControls({ dbId, canManage }: { dbId: string; canManage: boolean }) {
  const client = useQueryClient();
  const removedKey = ['db', dbId, 'removed-rejects'] as const;
  const removed = useQuery({ queryKey: removedKey, queryFn: () => apiClient.getRemovedRejects(dbId), enabled: canManage, staleTime: 60_000, retry: false });
  const [days, setDays] = useState(DEFAULT_DAYS);
  const [retention, setRetention] = useState(DEFAULT_RETENTION);
  const [plan, setPlan] = useState<RejectRemovalPlan | null>(null);
  const [notice, setNotice] = useState('');
  const refresh = () => {
    void client.invalidateQueries({ queryKey: removedKey });
    void client.invalidateQueries({ queryKey: ['db', dbId] });
  };
  const preview = useMutation({
    retry: false,
    mutationFn: () => apiClient.previewRejectRemoval(dbId, days),
    onSuccess: next => { setPlan(next); setNotice(''); },
  });
  const apply = useMutation({
    retry: false,
    mutationFn: (current: RejectRemovalPlan) => apiClient.applyRejectRemoval(dbId, current.min_age_days, current.digest, retention),
    onSuccess: report => {
      setPlan(null);
      setNotice(`Removed ${frames(report.removed.length)}; their files wait in the trash until ${day(report.trash_until)}.`
        + (report.failed.length ? ` ${frames(report.failed.length)} stayed: ${report.failed[0].reason}` : ''));
      refresh();
    },
    onError: error => { if (httpStatus(error) === 409) { setPlan(null); setNotice('The rejects changed since the preview; preview again.'); } },
  });
  const restore = useMutation({
    retry: false,
    mutationFn: (batch: string) => apiClient.restoreRemovedRejects(dbId, batch),
    onSuccess: report => {
      setNotice(`Restored ${frames(report.restored.length)}.` + (report.failed.length ? ` ${frames(report.failed.length)} could not come back: ${report.failed[0].reason}` : ''));
      refresh();
    },
  });
  const purge = useMutation({
    retry: false,
    mutationFn: (batch: string) => apiClient.purgeRemovedRejects(dbId, batch),
    onSuccess: report => { setNotice(`Purged the saved records of ${frames(report.frames)}; they can no longer be restored.`); refresh(); },
  });
  const empty = useMutation({
    retry: false,
    mutationFn: () => apiClient.emptyRejectTrash(dbId),
    onSuccess: report => { setNotice(`Deleted ${report.files_deleted.toLocaleString()} file${report.files_deleted === 1 ? '' : 's'} (${size(report.bytes)}) from the trash.`); refresh(); },
  });
  if (!canManage) return null;
  const now = Date.now() / 1000;
  const batches = removed.data?.batches ?? [];
  const emptiable = batches.some(batch => batch.files_deleted < batch.frames && batch.trash_until <= now);
  // Why rejects stay, counted: a shared file names the other frame, so those group as one.
  const reasons = plan ? Object.entries(plan.skipped.reduce<Record<string, number>>((counts, skip) => {
    const reason = skip.reason.startsWith('its file also backs') ? 'their file backs another frame' : skip.reason;
    counts[reason] = (counts[reason] ?? 0) + 1;
    return counts;
  }, {})) : [];
  const busy = preview.isPending || apply.isPending || restore.isPending || empty.isPending || purge.isPending;
  const error = preview.error ?? (apply.error && httpStatus(apply.error) !== 409 ? apply.error : null) ?? restore.error ?? empty.error ?? purge.error;
  return (
    <div className="quality-backfill-option reject-removal" role="group" aria-label="Remove rejects">
      <div className="reject-removal-row">
        <strong>Remove rejects</strong>
        <label>
          rejected
          <input type="number" min={0} max={3650} step={1} aria-label="Days rejected" value={days} disabled={busy}
            onChange={event => { setDays(Math.max(0, Math.round(Number(event.target.value) || 0))); setPlan(null); }} />
          days or more
        </label>
        <button type="button" className="browse-button" disabled={busy} onClick={() => preview.mutate()}>
          {preview.isPending ? 'Looking…' : 'Preview'}
        </button>
      </div>
      <small>Removed frames leave the catalog; their files wait in a trash folder, and can be restored, until the trash is emptied.</small>
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
            <button type="button" className="browse-button" disabled={busy} onClick={() => apply.mutate(plan)}>
              {apply.isPending ? 'Removing…' : `Remove ${frames(plan.frames.length)}`}
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
            {!gone && <button type="button" className="browse-button" disabled={busy} aria-label={`Restore the rejects removed ${day(batch.removed_at)}`} onClick={() => restore.mutate(batch.batch_id)}>Restore</button>}
            {gone && !purged && <button type="button" className="browse-button" disabled={busy} aria-label={`Purge the rejects removed ${day(batch.removed_at)}`}
              title="Forget their saved records. A marker stays so they never come back." onClick={() => purge.mutate(batch.batch_id)}>Purge</button>}
          </li>;
        })}
      </ul>}
      {emptiable && <button type="button" className="browse-button" disabled={busy} onClick={() => empty.mutate()}>
        {empty.isPending ? 'Emptying…' : 'Empty trash'}
      </button>}
      {notice && <small role="status">{notice}</small>}
      {error && <small className="settings-error" role="alert">{message(error)}</small>}
    </div>
  );
}
