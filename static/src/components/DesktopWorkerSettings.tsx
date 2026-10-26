import { useEffect, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  desktopWorker,
  tauriFileSystem,
  type DesktopWorker,
  type DesktopWorkerSettings as Settings,
  type DesktopWorkerStatus,
} from '../utils/tauri';
import OpenLogsButton from './OpenLogsButton';
import UnsavedMark from './UnsavedMark';
import './DesktopWorkerSettings.css';

const KEY = ['desktopWorker'] as const;

const DEFAULTS: Settings = {
  server: '',
  name: null,
  files: 'remote',
  maps: [],
  work_dir: null,
  rc_astro: null,
  work: [],
  slots: 1,
};

/** The form's copy of the settings, with the work as two choices. */
interface Draft extends Settings {
  stack: boolean;
  rcAstro: boolean;
  token: string;
  startAtLogin: boolean;
}

function draftOf(worker: DesktopWorker): Draft {
  const settings = worker.settings ?? DEFAULTS;
  const everything = settings.work.length === 0;
  return {
    ...settings,
    stack: everything || settings.work.includes('stack'),
    rcAstro: everything || settings.work.some((work) => work !== 'stack'),
    token: '',
    startAtLogin: worker.start_at_login,
  };
}

/** The `--work` choices for the two boxes: none when both are selected. */
function workOf(draft: Draft): string[] {
  if (draft.stack && draft.rcAstro) return [];
  return [...(draft.stack ? ['stack'] : []), ...(draft.rcAstro ? ['rc-astro'] : [])];
}

function sameSettings(draft: Draft, worker: DesktopWorker): boolean {
  const saved = draftOf(worker);
  return JSON.stringify({ ...draft, token: '' }) === JSON.stringify(saved);
}

function statusText(status: DesktopWorkerStatus): string {
  switch (status.connection) {
    case 'connected':
      return status.paused ? `Paused. Connected to ${status.server}.` : `Connected to ${status.server}.`;
    case 'failed':
      return `Stopped: ${status.problem ?? 'the worker could not continue'}.`;
    case 'stopped':
      return 'Stopped.';
    default:
      return status.problem
        ? `Cannot reach ${status.server}. Trying again.`
        : `Connecting to ${status.server}.`;
  }
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Settings › Workers › Worker on this computer: run a worker for a PSF Guard
 * server. The worker runs in the menu bar or the system tray, a process of
 * its own, so it keeps running when this window is closed.
 */
export default function DesktopWorkerSettings() {
  const client = useQueryClient();
  const worker = useQuery({
    queryKey: KEY,
    queryFn: desktopWorker.get,
    refetchInterval: 3000,
  });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [notice, setNotice] = useState('');
  const [logError, setLogError] = useState('');
  const loaded = worker.data;
  useEffect(() => {
    if (loaded && draft === null) setDraft(draftOf(loaded));
  }, [loaded, draft]);
  const action = useMutation({
    mutationFn: (run: () => Promise<DesktopWorker>) => run(),
    onSuccess: (next) => client.setQueryData(KEY, next),
  });

  if (worker.isPending || (loaded && !draft)) return <p>Loading worker settings…</p>;
  if (worker.isError || !loaded || !draft) {
    return (
      <section className="settings-section" aria-label="Worker on this computer">
        <p className="error-text" role="alert">
          The worker settings could not be read: {errorText(worker.error)}
        </p>
        <button type="button" onClick={() => void worker.refetch()}>Try again</button>
      </section>
    );
  }

  if (!loaded.supported) {
    return (
      <section className="settings-section desktop-worker-settings" aria-label="Worker on this computer">
        <h3>Worker on this computer</h3>
        <p>
          On Linux, run the worker as the <code>psf-guard-worker</code> service.
          Follow “Run a worker as a Linux service” in the Workers guide.
        </p>
      </section>
    );
  }

  const status = loaded.status;
  // One worker per computer: while the Windows service runs one, the page
  // offers no second.
  const serviceRuns = loaded.service === 'running' || loaded.service === 'changing';
  const dirty = draft.token.trim() !== '' || !sameSettings(draft, loaded);
  const busy = action.isPending;
  const update = (change: Partial<Draft>) => setDraft({ ...draft, ...change });
  const save = (event: FormEvent) => {
    event.preventDefault();
    setNotice('');
    if (!draft.stack && !draft.rcAstro) {
      setNotice('Select at least one task type: Stack builds or RC-Astro tools.');
      return;
    }
    action.mutate(
      () => desktopWorker.save({
        server: draft.server,
        name: draft.name?.trim() ? draft.name.trim() : null,
        files: draft.files,
        maps: draft.files === 'shared' ? draft.maps : [],
        work_dir: draft.work_dir?.trim() ? draft.work_dir.trim() : null,
        rc_astro: draft.rc_astro,
        work: workOf(draft),
        slots: draft.slots,
        token: draft.token.trim() || null,
        start_at_login: draft.startAtLogin,
      }),
      {
        onSuccess: (saved) => {
          setDraft(draftOf(saved));
          setNotice(saved.status ? 'Worker settings saved. Restart the worker to use them.' : 'Worker settings saved.');
        },
      },
    );
  };
  const restart = () => action.mutate(async () => {
    await desktopWorker.stop();
    return desktopWorker.start();
  });
  const chooseFolder = async () => {
    const picked = await tauriFileSystem.pickPath({ title: 'Temporary task folder' });
    if (picked) update({ work_dir: picked });
  };
  const shared = draft.files === 'shared';

  return (
    <section className="settings-section desktop-worker-settings" aria-label="Worker on this computer">
      <UnsavedMark label="worker settings" dirty={dirty} />
      <h3>Worker on this computer</h3>
      <p>
        Use this computer to build stacks and run RC-Astro tools for a PSF Guard
        server. Enter that server&apos;s address and a worker token from its
        Settings › Workers. After you start the worker, it keeps running in the
        menu bar or system tray when you close this window.
      </p>

      <div className="desktop-worker-status" aria-label="Worker status" role="group">
        <p role="status">
          {status
            ? statusText(status)
            : loaded.service === 'running'
              ? 'The PSF Guard Worker Windows service is running.'
              : serviceRuns
                ? 'The PSF Guard Worker Windows service has not stopped. Stop it in Windows Services to use the desktop worker.'
                : 'Not running.'}
        </p>
        {status?.tasks.map((task) => (
          <p key={task.id} className="muted">
            {task.label}: {Math.round(task.fraction * 100)}%
          </p>
        ))}
        {serviceRuns && !status && (
          <p className="muted">
            To update the service, click Save worker settings, then run{' '}
            <code>psf-guard-cli worker-service install</code> in PowerShell opened
            as administrator. This command updates the service&apos;s copies of
            the app and settings, then restarts the service. The desktop worker
            controls do not control the Windows service.
          </p>
        )}
        <div className="db-row-actions">
          {status ? (
            <>
              <button
                type="button"
                className="browse-button"
                disabled={busy}
                onClick={() => action.mutate(() => desktopWorker.pause(!status.paused))}
              >
                {status.paused ? 'Resume worker' : 'Pause worker'}
              </button>
              <button type="button" className="browse-button" disabled={busy} onClick={restart}>
                Restart worker
              </button>
              <button
                type="button"
                className="cancel-button"
                disabled={busy}
                onClick={() => action.mutate(desktopWorker.stop)}
              >
                Stop worker
              </button>
            </>
          ) : (
            <button
              type="button"
              className="save-button"
              disabled={busy || dirty || serviceRuns || !loaded.settings || !loaded.has_token}
              onClick={() => action.mutate(desktopWorker.start)}
            >
              Start worker
            </button>
          )}
          <OpenLogsButton onError={setLogError} />
        </div>
        {!status && !serviceRuns && (!loaded.settings || !loaded.has_token) && (
          <p className="muted">Save a server address and a worker token before starting the worker.</p>
        )}
        {!status && !serviceRuns && loaded.settings && loaded.has_token && dirty && (
          <p className="muted">Save your changes before starting the worker.</p>
        )}
        {!serviceRuns && <p className="muted">
          Pause lets running tasks finish. A task already requested from the server
          may still start. Stop returns unfinished tasks to the server&apos;s queue.
        </p>}
      </div>

      <form className="user-form" onSubmit={save}>
        <div className="database-config">
          <label htmlFor="desktop-worker-server">Server address</label>
          <input
            id="desktop-worker-server"
            className="file-path-input"
            value={draft.server}
            placeholder="http://192.168.1.10:3000"
            onChange={(event) => update({ server: event.target.value })}
            required
            disabled={busy}
          />
        </div>
        <div className="database-config">
          <label htmlFor="desktop-worker-token">Worker token</label>
          <input
            id="desktop-worker-token"
            aria-describedby="desktop-worker-token-help"
            className="file-path-input"
            type="password"
            autoComplete="off"
            value={draft.token}
            placeholder={loaded.has_token ? 'Saved token' : 'psfw_…'}
            onChange={(event) => update({ token: event.target.value })}
            required={!loaded.has_token}
            disabled={busy}
          />
          <p className="muted" id="desktop-worker-token-help">{loaded.has_token
            ? 'Leave blank to keep the saved token.'
            : 'Create a worker token in the server’s Settings › Workers, then paste it here.'}</p>
        </div>
        <div className="database-config">
          <label htmlFor="desktop-worker-name">Worker name</label>
          <input
            id="desktop-worker-name"
            aria-describedby="desktop-worker-name-help"
            className="file-path-input"
            value={draft.name ?? ''}
            placeholder="This computer's name"
            maxLength={64}
            onChange={(event) => update({ name: event.target.value })}
            disabled={busy}
          />
          <p className="muted" id="desktop-worker-name-help">Leave blank to use this computer&apos;s host name.</p>
        </div>
        <fieldset className="desktop-worker-work">
          <legend>Task types</legend>
          <label>
            <input
              type="checkbox"
              checked={draft.stack}
              onChange={(event) => update({ stack: event.target.checked })}
              disabled={busy}
            />
            Stack builds
          </label>
          <label>
            <input
              type="checkbox"
              checked={draft.rcAstro}
              onChange={(event) => update({ rcAstro: event.target.checked })}
              disabled={busy}
            />
            RC-Astro tools
          </label>
        </fieldset>
        <div className="database-config">
          <label htmlFor="desktop-worker-slots">Concurrent tasks</label>
          <input
            id="desktop-worker-slots"
            aria-describedby="desktop-worker-slots-help"
            className="file-path-input"
            type="number"
            min={1}
            max={16}
            value={draft.slots}
            onChange={(event) => update({ slots: Number(event.target.value) })}
            disabled={busy}
          />
          <p className="muted" id="desktop-worker-slots-help">Maximum tasks this worker runs at the same time, from 1 to 16. Start with 1 for GPU work.</p>
        </div>
        <label className="desktop-worker-login">
          <input
            type="checkbox"
            checked={draft.startAtLogin}
            onChange={(event) => update({ startAtLogin: event.target.checked })}
            disabled={busy}
          />
          Start worker at login
        </label>
        <details className="desktop-worker-files" open={shared || Boolean(draft.work_dir)}>
          <summary>Task files</summary>
          <p className="muted">
            The worker downloads inputs from the server and uploads results by
            default. Use shared storage if both computers can access the same folders.
          </p>
          <label className="desktop-worker-login">
            <input
              type="checkbox"
              checked={shared}
              onChange={(event) => update(event.target.checked
                ? { files: 'shared', maps: draft.maps.length ? draft.maps : [{ server: '', local: '' }] }
                : { files: 'remote' })}
              disabled={busy}
            />
            Use shared storage
          </label>
          {shared && (
            <div className="desktop-worker-maps">
              <p className="muted">Enter each shared folder&apos;s path on the server and its path on this computer.</p>
              {draft.maps.map((map, index) => (
                <div className="token-secret-row" key={index}>
                  <label className="desktop-worker-map-field">Server folder {index + 1}<input
                    className="file-path-input"
                    aria-label={`Server folder ${index + 1}`}
                    placeholder="/srv/astro"
                    value={map.server}
                    onChange={(event) => update({
                      maps: draft.maps.map((each, at) => at === index ? { ...each, server: event.target.value } : each),
                    })}
                    disabled={busy}
                  /></label>
                  <label className="desktop-worker-map-field">Folder on this computer {index + 1}<input
                    className="file-path-input"
                    aria-label={`Folder on this computer ${index + 1}`}
                    placeholder="Z:\astro"
                    value={map.local}
                    onChange={(event) => update({
                      maps: draft.maps.map((each, at) => at === index ? { ...each, local: event.target.value } : each),
                    })}
                    disabled={busy}
                  /></label>
                  <button
                    type="button"
                    className="cancel-button"
                    aria-label={`Remove folder mapping ${index + 1}`}
                    onClick={() => update({ maps: draft.maps.filter((_, at) => at !== index) })}
                    disabled={busy}
                  >
                    Remove mapping
                  </button>
                </div>
              ))}
              <button
                type="button"
                className="browse-button"
                onClick={() => update({ maps: [...draft.maps, { server: '', local: '' }] })}
                disabled={busy}
              >
                Add folder mapping
              </button>
            </div>
          )}
          <div className="database-config">
            <label htmlFor="desktop-worker-folder">Temporary task folder</label>
            <div className="token-secret-row">
              <input
                id="desktop-worker-folder"
                aria-describedby="desktop-worker-folder-help"
                className="file-path-input"
                value={draft.work_dir ?? ''}
                placeholder="System temporary folder"
                onChange={(event) => update({ work_dir: event.target.value })}
                disabled={busy}
              />
              <button type="button" className="browse-button" onClick={() => void chooseFolder()} disabled={busy}>
                Choose folder…
              </button>
            </div>
            <p className="muted" id="desktop-worker-folder-help">Leave blank to use the system temporary folder. The worker stores task files here while tasks run.</p>
            {loaded.service != null && <p className="muted">
              The Windows service uses <code>%ProgramData%\PSF Guard Worker\tasks</code> instead.
              Temporary task folder applies to the desktop worker.
            </p>}
          </div>
        </details>
        <div className="modal-buttons">
          <button type="submit" className="save-button" disabled={busy || !dirty}>
            {busy ? 'Saving…' : 'Save worker settings'}
          </button>
        </div>
      </form>
      {(notice || action.error || logError) && (
        <p className={action.error || logError ? 'error-text' : 'muted'} role={action.error || logError ? 'alert' : 'status'}>
          {action.error ? errorText(action.error) : logError || notice}
        </p>
      )}
    </section>
  );
}
