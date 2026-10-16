import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import {
  BACKUP_STATUS_QUERY_KEY,
  BACKUPS_QUERY_KEY,
  type BackupJob,
  type BackupLastRun,
  type BackupListing,
  type BackupRetention,
  type BackupScope,
  type BackupSettings as Schedule,
  type BackupsOverview,
} from '../api/backupTypes';
import { isTauriApp } from '../utils/tauri';
import { describeJob, jobPercent } from '../utils/backupJob';
import PathField from './PathField';
import './BackupSettings.css';

const KB = 1000;
const MB = KB * 1000;
const GB = MB * 1000;

function formatBytes(count: number): string {
  if (count >= GB) return `${(count / GB).toFixed(1)} GB`;
  if (count >= MB) return `${(count / MB).toFixed(1)} MB`;
  if (count >= KB) return `${Math.round(count / KB)} KB`;
  return `${count} B`;
}

function when(unix: number): string {
  return new Date(unix * 1000).toLocaleString([], {
    dateStyle: 'medium',
    timeStyle: 'short',
  });
}

const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

function secretsList(overview: BackupsOverview): string[] {
  const { peer_keys, collaboration_credentials, server_config_tokens } = overview.secrets;
  return [
    peer_keys > 0 ? `${peer_keys} peer key${peer_keys === 1 ? '' : 's'}` : '',
    collaboration_credentials ? 'collaboration sign-ins' : '',
    server_config_tokens > 0
      ? `${server_config_tokens} token${server_config_tokens === 1 ? '' : 's'} in the server config file`
      : '',
  ].filter(Boolean);
}

/** Say what a folder means, as the server sees it, a moment after typing stops. */
function useFolderCheck(path: string) {
  const [settled, setSettled] = useState(path.trim());
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(path.trim()), 400);
    return () => window.clearTimeout(timer);
  }, [path]);
  return useQuery({
    queryKey: ['backups', 'check-destination', settled],
    queryFn: () => apiClient.checkBackupDestination(settled),
    enabled: settled !== '',
    staleTime: 30_000,
  });
}

function FolderNotes({ path }: { path: string }) {
  const check = useFolderCheck(path);
  if (!path.trim() || !check.data) return null;
  return (
    <>
      {check.data.error && (
        <p className="backup-note is-bad" role="alert">{check.data.error}</p>
      )}
      {check.data.warnings.map((warning) => (
        <p key={warning} className="backup-note is-caution">{warning}</p>
      ))}
    </>
  );
}

function SecretsToggle({
  overview,
  checked,
  onChange,
}: {
  overview: BackupsOverview;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  const held = secretsList(overview);
  return (
    <label className="review-preference">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <span>
        Include secrets
        <small>
          {held.length > 0
            ? `PSF Guard holds ${held.join(', ')}. Left out, you enter them again after a restore. Included, anyone with the archive can use them.`
            : 'PSF Guard holds no peer keys or tokens now. Account passwords are kept as hashes either way.'}
        </small>
      </span>
    </label>
  );
}

function ScopeChoice({
  overview,
  value,
  onChange,
  name,
}: {
  overview: BackupsOverview;
  value: BackupScope;
  onChange: (value: BackupScope) => void;
  name: string;
}) {
  const metaSize = formatBytes(overview.meta_bytes);
  const fullSize = formatBytes(overview.meta_bytes + overview.catalog_bytes);
  return (
    <fieldset className="calibration-settings-group backup-scope">
      <legend>What to back up</legend>
      <label className="review-preference">
        <input
          type="radio"
          name={name}
          checked={value === 'full'}
          onChange={() => onChange('full')}
        />
        <span>
          Settings and catalogs
          <small>
            Everything below, plus {overview.catalogs === 1 ? 'the catalog' : `all ${overview.catalogs} catalogs`},
            copied safely while N.I.N.A. keeps writing. About {fullSize} before compression.
          </small>
        </span>
      </label>
      <label className="review-preference">
        <input
          type="radio"
          name={name}
          checked={value === 'meta'}
          onChange={() => onChange('meta')}
        />
        <span>
          Settings only
          <small>
            The database list and settings, accounts, preferences, processing setups and
            Planning&apos;s store. About {metaSize}.
          </small>
        </span>
      </label>
      {value === 'full' && overview.unreachable_catalogs.length > 0 && (
        <p className="backup-note is-caution">
          Not reachable now, so left out: {overview.unreachable_catalogs.join(', ')}.
        </p>
      )}
    </fieldset>
  );
}

function JobProgress({ job, onStop, stopping }: { job: BackupJob; onStop: () => void; stopping: boolean }) {
  const percent = jobPercent(job);
  return (
    <div className="backup-job" role="status">
      <div className="backup-job-head">
        <strong>{job.trigger === 'schedule' ? 'Scheduled backup' : 'Backing up'}</strong>
        <span className="muted">{describeJob(job)}</span>
      </div>
      <div
        className="backup-job-bar"
        role="progressbar"
        aria-label="Backup progress"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent == null ? undefined : Math.round(percent)}
      >
        <span style={{ width: `${percent ?? 0}%` }} />
      </div>
      <button
        type="button"
        className="header-button"
        onClick={onStop}
        disabled={stopping || job.stopping}
      >
        Stop
      </button>
    </div>
  );
}

function LastRunNote({ run, listing }: { run: BackupLastRun; listing?: BackupListing }) {
  const finished = when(run.finished_at);
  const where = run.location === 'downloads' ? 'ready to download' : run.destination;
  if (run.outcome === 'failed') {
    return (
      <p className="backup-note is-bad" role="alert">
        The backup of {finished} failed: {run.error}
      </p>
    );
  }
  if (run.outcome === 'stopped') {
    return <p className="backup-note">The backup of {finished} was stopped; nothing was kept.</p>;
  }
  return (
    <div className="backup-note is-good">
      <span>
        Backed up {finished}: <code>{run.file_name}</code>, {formatBytes(run.bytes ?? 0)}, {where}.
        {run.pruned.length > 0 && ` Removed ${run.pruned.length} older.`}
      </span>
      {listing && !isTauriApp() && (
        <a className="header-button" href={apiClient.backupDownloadUrl(listing.download_path)} download>
          Download
        </a>
      )}
      {run.problems.map((problem) => (
        <span key={problem} className="backup-note is-caution">Left out: {problem}</span>
      ))}
    </div>
  );
}

function BackupNow({ overview, job }: { overview: BackupsOverview; job: BackupJob | null }) {
  const queryClient = useQueryClient();
  const desktop = isTauriApp();
  const [scope, setScope] = useState<BackupScope>('full');
  const [secrets, setSecrets] = useState(false);
  // The desktop app has no download: it saves to a folder, the schedule's
  // when there is one.
  const [folder, setFolder] = useState(() =>
    desktop ? overview.settings.destination ?? overview.suggested_destination : ''
  );
  const run = useMutation({
    mutationFn: apiClient.runBackup,
    onSuccess: (view) => queryClient.setQueryData(BACKUP_STATUS_QUERY_KEY, view),
  });
  const stop = useMutation({
    mutationFn: apiClient.stopBackup,
    onSuccess: (view) => queryClient.setQueryData(BACKUP_STATUS_QUERY_KEY, view),
  });
  const last = overview.last_run;
  const lastListing = last?.file_name
    ? overview.backups.find((listing) => listing.file_name === last.file_name)
    : undefined;
  const needsFolder = desktop && !folder.trim();

  return (
    <section className="backup-card" aria-labelledby="backup-now-heading">
      <h3 id="backup-now-heading">Back up now</h3>
      <p className="review-preferences-note">
        One zip archive with a manifest of every file, its checksum and where it goes back. Previews,
        stacks and masters are left out: PSF Guard makes them again.
      </p>
      <ScopeChoice overview={overview} value={scope} onChange={setScope} name="backup-now-scope" />
      <SecretsToggle overview={overview} checked={secrets} onChange={setSecrets} />
      <label className="backup-folder" htmlFor="backup-now-folder">
        <span>
          {desktop ? 'Save to' : 'Save on the server'}
          <small className="muted">
            {desktop
              ? 'A folder on another disk keeps the backup safe if this one fails.'
              : 'Empty: download the archive in this browser.'}
          </small>
        </span>
        <PathField
          id="backup-now-folder"
          dialogTitle="Select a folder for the backup"
          placeholder={desktop ? overview.suggested_destination : 'Download in the browser'}
          value={folder}
          onChange={setFolder}
        />
      </label>
      <FolderNotes path={folder} />
      <div className="settings-actions">
        <button
          type="button"
          className="save-button"
          disabled={!!job || run.isPending || needsFolder}
          onClick={() =>
            run.mutate({ scope, include_secrets: secrets, destination: folder.trim() || null })
          }
        >
          Back up now
        </button>
      </div>
      {run.isError && <p className="error-text" role="alert">{(run.error as Error).message}</p>}
      {stop.isError && <p className="error-text" role="alert">{(stop.error as Error).message}</p>}
      {job ? (
        <JobProgress job={job} onStop={() => stop.mutate()} stopping={stop.isPending} />
      ) : (
        last && <LastRunNote run={last} listing={lastListing} />
      )}
    </section>
  );
}

function retentionCount(retention: BackupRetention): number {
  return retention.kind === 'count' ? retention.count : retention.days;
}

function BackupSchedule({ overview }: { overview: BackupsOverview }) {
  const queryClient = useQueryClient();
  const saved = overview.settings;
  const [draft, setDraft] = useState<Schedule>(saved);
  const [keep, setKeep] = useState(String(retentionCount(saved.retention)));
  // Follow a save made elsewhere unless this form has edits of its own.
  const savedKey = JSON.stringify(saved);
  const lastSaved = useRef(savedKey);
  useEffect(() => {
    if (lastSaved.current === savedKey) return;
    const previous = lastSaved.current;
    lastSaved.current = savedKey;
    setDraft((current) => (JSON.stringify(current) === previous ? saved : current));
    setKeep(String(retentionCount(saved.retention)));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- follows the saved value by its key.
  }, [savedKey]);
  const save = useMutation({
    mutationFn: apiClient.updateBackupSettings,
    onSuccess: (updated) => {
      lastSaved.current = JSON.stringify(updated.settings);
      setDraft(updated.settings);
      setKeep(String(retentionCount(updated.settings.retention)));
      queryClient.setQueryData(BACKUPS_QUERY_KEY, updated);
    },
  });
  const keepNumber = Number(keep);
  const keepValid = Number.isInteger(keepNumber) && keepNumber >= 1 && keepNumber <= (draft.retention.kind === 'count' ? 1000 : 3650);
  const next: Schedule = {
    ...draft,
    retention:
      draft.retention.kind === 'count'
        ? { kind: 'count', count: keepValid ? keepNumber : 0 }
        : { kind: 'age', days: keepValid ? keepNumber : 0 },
  };
  const dirty = JSON.stringify(next) !== JSON.stringify(saved);
  const folderMissing = draft.enabled && !(draft.destination ?? '').trim();
  const set = (change: Partial<Schedule>) => setDraft((current) => ({ ...current, ...change }));
  const lastScheduled = overview.last_run?.trigger === 'schedule' ? overview.last_run : null;
  const due = overview.next_run_at != null && overview.next_run_at * 1000 <= Date.now();

  return (
    <section className="backup-card" aria-labelledby="backup-schedule-heading">
      <h3 id="backup-schedule-heading">Scheduled backups</h3>
      <p className="review-preferences-note">
        Runs in the background at the time you choose, steps aside for work you start, and removes
        only its own older backups. Midday suits most rigs: catalogs are busy at night.
      </p>
      <label className="review-preference">
        <input
          type="checkbox"
          checked={draft.enabled}
          onChange={(event) =>
            set({
              enabled: event.target.checked,
              destination:
                event.target.checked && !(draft.destination ?? '').trim()
                  ? overview.suggested_destination
                  : draft.destination,
            })
          }
        />
        <span>
          Back up on a schedule
          <small>Off by default.</small>
        </span>
      </label>
      <fieldset className="calibration-settings-group calibration-matching-group" disabled={!draft.enabled}>
        <legend>When</legend>
        <label className="review-preference">
          <span>Every</span>
          <select
            value={draft.frequency}
            aria-label="How often"
            onChange={(event) => set({ frequency: event.target.value as Schedule['frequency'] })}
          >
            <option value="daily">Day</option>
            <option value="weekly">Week</option>
          </select>
        </label>
        {draft.frequency === 'weekly' && (
          <label className="review-preference">
            <span>On</span>
            <select
              value={draft.weekday}
              aria-label="Weekday"
              onChange={(event) => set({ weekday: Number(event.target.value) })}
            >
              {WEEKDAYS.map((day, index) => (
                <option key={day} value={index}>{day}</option>
              ))}
            </select>
          </label>
        )}
        <label className="review-preference">
          <span>
            At
            <small>The server&apos;s local time.</small>
          </span>
          <input
            type="time"
            step={60}
            aria-label="Time of day"
            value={draft.time}
            onChange={(event) => set({ time: event.target.value })}
          />
        </label>
      </fieldset>
      <fieldset className="calibration-settings-group" disabled={!draft.enabled}>
        <legend>Where and what</legend>
        <label className="backup-folder" htmlFor="backup-schedule-folder">
          <span>
            Folder
            <small className="muted">
              Best on another disk. The schedule only ever removes archives it made itself.
            </small>
          </span>
          <PathField
            id="backup-schedule-folder"
            dialogTitle="Select the folder for scheduled backups"
            placeholder={overview.suggested_destination}
            value={draft.destination ?? ''}
            onChange={(destination) => set({ destination })}
          />
        </label>
        <FolderNotes path={draft.enabled ? draft.destination ?? '' : ''} />
      </fieldset>
      <ScopeChoice
        overview={overview}
        value={draft.scope}
        onChange={(scope) => set({ scope })}
        name="backup-schedule-scope"
      />
      <SecretsToggle
        overview={overview}
        checked={draft.include_secrets}
        onChange={(include_secrets) => set({ include_secrets })}
      />
      <fieldset className="calibration-settings-group calibration-matching-group">
        <legend>How many to keep</legend>
        <label className="review-preference">
          <span>
            Keep
            <small>
              {draft.retention.kind === 'count'
                ? 'Older scheduled backups go once there are more.'
                : 'Older scheduled backups go; the newest always stays.'}
            </small>
          </span>
          <span className="backup-keep">
            <select
              aria-label="Keep by"
              value={draft.retention.kind}
              onChange={(event) =>
                set({
                  retention:
                    event.target.value === 'count'
                      ? { kind: 'count', count: keepValid ? keepNumber : 7 }
                      : { kind: 'age', days: keepValid ? keepNumber : 30 },
                })
              }
            >
              <option value="count">The newest</option>
              <option value="age">Days</option>
            </select>
            <input
              type="number"
              min={1}
              max={draft.retention.kind === 'count' ? 1000 : 3650}
              aria-label={draft.retention.kind === 'count' ? 'Backups to keep' : 'Days to keep'}
              aria-invalid={!keepValid}
              value={keep}
              onChange={(event) => setKeep(event.target.value)}
            />
          </span>
        </label>
      </fieldset>
      <div className="settings-actions">
        <button
          type="button"
          className="save-button"
          disabled={!dirty || !keepValid || folderMissing || save.isPending}
          onClick={() => save.mutate(next)}
        >
          {save.isPending ? 'Saving…' : 'Save schedule'}
        </button>
      </div>
      {save.isError && <p className="error-text" role="alert">{(save.error as Error).message}</p>}
      <dl className="backup-schedule-status">
        <dt>Next run</dt>
        <dd>
          {overview.next_run_at == null
            ? 'Not scheduled'
            : due
              ? 'Due now'
              : when(overview.next_run_at)}
        </dd>
        <dt>Last scheduled run</dt>
        <dd>
          {lastScheduled
            ? `${when(lastScheduled.finished_at)} · ${
                lastScheduled.outcome === 'succeeded'
                  ? `succeeded, ${formatBytes(lastScheduled.bytes ?? 0)}`
                  : lastScheduled.outcome === 'failed'
                    ? `failed: ${lastScheduled.error ?? ''}`
                    : 'stopped'
              }`
            : 'None since the server started'}
        </dd>
      </dl>
      {saved.enabled && overview.destination_warnings.map((warning) => (
        <p key={warning} className="backup-note is-caution">{warning}</p>
      ))}
    </section>
  );
}

function BackupList({ overview }: { overview: BackupsOverview }) {
  const desktop = isTauriApp();
  return (
    <section className="backup-card" aria-labelledby="backup-list-heading">
      <h3 id="backup-list-heading">Backups</h3>
      {overview.backups.length === 0 ? (
        <p className="review-preferences-note">
          No backups yet in the schedule&apos;s folder or the server&apos;s download area.
        </p>
      ) : (
        <ul className="backup-list">
          {overview.backups.map((listing) => (
            <li key={`${listing.location}/${listing.file_name}`} className="backup-row">
              <span className="backup-row-what">
                <strong>{when(listing.created_unix)}</strong>
                <span className="muted">
                  {listing.scope === 'full'
                    ? `Settings and ${listing.catalogs} catalog${listing.catalogs === 1 ? '' : 's'}`
                    : 'Settings only'}
                  {' · '}
                  {listing.trigger === 'schedule' ? 'scheduled' : listing.trigger === 'cli' ? 'command line' : 'manual'}
                  {listing.secrets === 'included' ? ' · with secrets' : ''}
                  {listing.location === 'downloads' ? ' · download area' : ''}
                </span>
                <code className="backup-row-file">{listing.file_name}</code>
              </span>
              <span className="backup-row-size">{formatBytes(listing.bytes)}</span>
              {!desktop && (
                <a
                  className="header-button"
                  href={apiClient.backupDownloadUrl(listing.download_path)}
                  download
                  aria-label={`Download ${listing.file_name}`}
                >
                  Download
                </a>
              )}
            </li>
          ))}
        </ul>
      )}
      <p className="review-preferences-note">
        To restore, stop PSF Guard and copy the files back by hand; the archive&apos;s README.txt and
        manifest say where each goes.
      </p>
    </section>
  );
}

/**
 * Settings › Backups: back up PSF Guard's settings and catalogs now or on a
 * schedule, and list the archives. Behind the database-management gate.
 */
export default function BackupSettings() {
  const queryClient = useQueryClient();
  const overview = useQuery({
    queryKey: BACKUPS_QUERY_KEY,
    queryFn: apiClient.getBackups,
    refetchInterval: 60_000,
  });
  // Shares its cache with the job line.
  const status = useQuery({
    queryKey: BACKUP_STATUS_QUERY_KEY,
    queryFn: apiClient.getBackupStatus,
    refetchInterval: (query) => (query.state.data?.job ? 1000 : 15_000),
  });
  const job = status.data ? status.data.job : overview.data?.job ?? null;
  // A run that ends changes the list and the last run.
  const running = !!job;
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) {
      queryClient.invalidateQueries({ queryKey: BACKUPS_QUERY_KEY, exact: true });
    }
    wasRunning.current = running;
  }, [running, queryClient]);

  if (overview.isLoading) return null;
  if (overview.isError || !overview.data) {
    return (
      <div className="settings-setups">
        <div>
          <h3>Backups</h3>
          <p className="error-text" role="alert">
            {(overview.error as Error | null)?.message ?? 'Could not load backups.'}
          </p>
        </div>
      </div>
    );
  }
  const data: BackupsOverview = {
    ...overview.data,
    last_run: status.data?.last_run ?? overview.data.last_run,
  };
  return (
    <div className="settings-setups backup-settings">
      <BackupNow overview={data} job={job} />
      <BackupSchedule overview={data} />
      <BackupList overview={data} />
    </div>
  );
}
