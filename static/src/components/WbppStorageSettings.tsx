import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WbppStoragePolicy } from '../api/types';
import PathField from './PathField';
import UnsavedMark from './UnsavedMark';
import { formatFree } from '../utils/pixinsight';

const DEFAULTS: WbppStoragePolicy = { max_volume_percent: 90, intermediate_days: 7, master_days: 90 };

export default function WbppStorageSettings({ canManage }: { canManage: boolean }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ['pixinsight-settings'],
    queryFn: apiClient.getPixInsightSettings,
    refetchInterval: 60_000,
  });
  const [draft, setDraft] = useState<WbppStoragePolicy | null>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: apiClient.updatePixInsightSettings,
    onSuccess: (updated) => {
      client.setQueryData(['pixinsight-settings'], updated);
      setDraft(null);
      setFolder(null);
    },
  });
  if (query.isLoading) return null;
  if (!query.data) return <p role="alert">Could not load WBPP storage settings.</p>;
  const current = query.data;
  const saved = current.storage ?? DEFAULTS;
  const policy = draft ?? saved;
  const path = folder ?? current.runs_dir ?? '';
  const changed =
    JSON.stringify(policy) !== JSON.stringify(saved) || path.trim() !== (current.runs_dir ?? '');
  const valid =
    Number.isInteger(policy.max_volume_percent) &&
    policy.max_volume_percent >= 0 &&
    policy.max_volume_percent <= 100 &&
    Number.isInteger(policy.intermediate_days) &&
    policy.intermediate_days >= 1 &&
    policy.intermediate_days <= 36500 &&
    Number.isInteger(policy.master_days) &&
    (policy.master_days === 0 ||
      (policy.master_days >= policy.intermediate_days && policy.master_days <= 36500));
  const edit = (field: keyof WbppStoragePolicy, value: number) => setDraft({ ...policy, [field]: value });
  const report = current.storage_report;
  return (
    <div className="storage-folders">
      <h3>WBPP storage</h3>
      <UnsavedMark label="WBPP storage" dirty={changed} />
      <p className="muted">
        Keep WBPP work files in their own folder. Cleanup checks every five minutes and removes intermediate
        files first. Active runs, downloads and master copies are protected.
      </p>
      <fieldset disabled={!canManage || save.isPending}>
        <label className="review-preference">
          <span>
            WBPP runs folder
            <small>
              Changing this folder affects new runs. Old folders remain managed. Leave blank to use each
              database’s export directory, or its stack folder.
            </small>
          </span>
          <PathField
            aria-label="WBPP storage folder"
            dialogTitle="Select the WBPP runs folder"
            value={path}
            onChange={setFolder}
            placeholder="/data/wbpp-runs"
            disabled={!canManage || save.isPending}
          />
        </label>
        <label className="review-preference worker-share">
          <span>
            WBPP volume usage limit
            <em className="worker-share-value">
              {policy.max_volume_percent === 100 ? 'no limit' : `${policy.max_volume_percent}%`}
            </em>
            <small>
              Total volume usage, including other files. A shared volume uses the lower storage limit. Above
              this limit, cleanup removes older intermediate files toward two percentage points below it.
              Masters keep their retention period. 100% disables pressure cleanup.
            </small>
          </span>
          <input
            aria-label="WBPP volume usage limit, in percent"
            type="range"
            min={0}
            max={100}
            step={1}
            value={policy.max_volume_percent}
            onChange={(e) => edit('max_volume_percent', Number(e.target.value))}
          />
        </label>
        <label className="review-preference">
          <span>
            Intermediate retention (days)
            <small>
              Calibrated and registered frames, scripts and logs expire after this many days. Disk pressure
              can remove them sooner.
            </small>
          </span>
          <input
            aria-label="WBPP intermediate retention, in days"
            type="number"
            min={1}
            max={36500}
            value={policy.intermediate_days}
            onChange={(e) => edit('intermediate_days', Number(e.target.value))}
          />
        </label>
        <label className="review-preference">
          <span>
            Master retention (days)
            <small>
              Masters expire after this period, measured from the end of the run or their last write. Copies
              saved to a process directory and imported stacks are separate.
            </small>
          </span>
          <input
            aria-label="WBPP master retention, in days"
            type="number"
            min={policy.intermediate_days}
            max={36500}
            disabled={policy.master_days === 0}
            value={policy.master_days || 90}
            onChange={(e) => edit('master_days', Number(e.target.value))}
          />
        </label>
        <label className="review-preference">
          <input
            type="checkbox"
            checked={policy.master_days === 0}
            onChange={(e) =>
              edit('master_days', e.target.checked ? 0 : Math.max(90, policy.intermediate_days))
            }
          />
          <span>
            Keep WBPP masters forever
            <small>Cleanup will not delete masters, even when the volume exceeds its limit.</small>
          </span>
        </label>
      </fieldset>
      {!valid && (
        <p role="alert">
          Use whole days between 1 and 36500. Keep masters at least as long as intermediate files, or choose
          forever.
        </p>
      )}
      <button
        type="button"
        className="save-button"
        disabled={!canManage || !changed || !valid || save.isPending}
        onClick={() =>
          save.mutate({ binary: current.binary, runs_dir: path.trim() || null, storage: policy })
        }
      >
        Save WBPP storage
      </button>
      {save.isError && (
        <p className="error-text" role="alert">
          {(save.error as Error).message}
        </p>
      )}
      {report?.checked_at != null && (
        <p className="muted">
          {report.runs} runs · intermediate files {formatFree(report.intermediate_bytes)} · masters{' '}
          {formatFree(report.master_bytes)}. Last checked{' '}
          {new Date(report.checked_at * 1000).toLocaleString()}. Last cleanup deleted {report.deleted_files}{' '}
          files ({formatFree(report.freed_bytes)}).
        </p>
      )}
      {report?.volumes.map((volume) => (
        <p key={volume.path} className={volume.over_limit ? 'error-text' : 'muted'}>
          <code>{volume.path}</code>: {volume.used_percent.toFixed(1)}% used (cleanup limit{' '}
          {volume.max_percent}%).{' '}
          {volume.over_limit &&
            'The volume is still above its WBPP limit. Retained masters, active work, and unrelated files may prevent further cleanup.'}
        </p>
      ))}
      {report?.errors.map((error) => (
        <p key={error} className="error-text" role="alert">
          {error}
        </p>
      ))}
    </div>
  );
}
