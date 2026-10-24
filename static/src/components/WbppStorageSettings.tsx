import type { WbppStoragePolicy, WbppStorageReport as Report } from '../api/types';
import { formatFree } from '../utils/pixinsight';
import { validWbppStorage } from '../utils/storageSettings';

export default function WbppStorageSettings({
  policy,
  onChange,
  disabled,
}: {
  policy: WbppStoragePolicy;
  onChange: (field: keyof WbppStoragePolicy, value: number) => void;
  disabled: boolean;
}) {
  const edit = onChange;
  return (
    <fieldset className="settings-group" disabled={disabled}>
      <legend>WBPP retention</legend>
      <label className="review-preference">
        <span>
          Intermediate retention (days)
          <small>
            Calibrated and registered frames, scripts and logs expire after this many days. Disk pressure can
            remove them sooner.
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
          value={policy.master_days === 0 ? 90 : Number.isNaN(policy.master_days) ? '' : policy.master_days}
          onChange={(e) =>
            edit(
              'master_days',
              e.target.value !== '' && Number(e.target.value) > 0 ? Number(e.target.value) : Number.NaN
            )
          }
        />
      </label>
      <label className="review-preference">
        <input
          type="checkbox"
          checked={policy.master_days === 0}
          onChange={(e) => edit('master_days', e.target.checked ? 0 : Math.max(90, policy.intermediate_days))}
        />
        <span>
          Keep WBPP masters forever
          <small>Cleanup will not delete masters, even when the volume exceeds its limit.</small>
        </span>
      </label>
      {!validWbppStorage(policy) && (
        <p role="alert">
          Use whole days between 1 and 36500. Keep masters at least as long as intermediate files, or choose
          forever.
        </p>
      )}
    </fieldset>
  );
}

export function WbppStorageReport({ report }: { report?: Report }) {
  return (
    <div className="storage-folders">
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
