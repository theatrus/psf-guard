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

/** The `--work` choices for the two boxes: none when both are ticked. */
function workOf(draft: Draft): string[] {
  if (draft.stack && draft.rcAstro) return [];
  return [...(draft.stack ? ['stack'] : []), ...(draft.rcAstro ? ['rc-astro'] : [])];
}

function sameSettings(draft: Draft, worker: DesktopWorker): boolean {
  const saved = draftOf(worker);
  return JSON.stringify({ ...draft, token: '' }) === JSON.stringify(saved);
}

function connectionText(status: DesktopWorkerStatus): string {
  switch (status.connection) {
    case 'connected':
      return `Connected to ${status.server} as ${status.name}`;
    case 'refused':
      return `Stopped: ${status.problem ?? 'the server refused the worker'}`;
    case 'stopped':
      return 'Stopped';
    default:
      return status.problem
        ? `Cannot reach ${status.server}; trying again. ${status.problem}`
        : `Connecting to ${status.server}`;
  }
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Settings › Run a worker: this computer as a worker for a PSF Guard
 * server. The worker runs in the menu bar or the system tray, a process of
 * its own, so it keeps working while this window is closed.
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
  const publish = (next: DesktopWorker) => client.setQueryData(KEY, next);
  const action = useMutation({
    mutationFn: (run: () => Promise<DesktopWorker>) => run(),
    onSuccess: publish,
  });

  if (worker.isPending || (loaded && !draft)) return <p>Loading the worker…</p>;
  if (worker.isError || !loaded || !draft) {
    return (
      <div className="settings-section">
        <p className="error-text" role="alert">
          The worker settings could not be read: {errorText(worker.error)}
        </p>
        <button type="button" onClick={() => void worker.refetch()}>Try again</button>
      </div>
    );
  }
  const status = loaded.status;
  const dirty = draft.token.trim() !== '' || !sameSettings(draft, loaded);
  const busy = action.isPending;
  const update = (change: Partial<Draft>) => setDraft({ ...draft, ...change });
  const save = (event: FormEvent) => {
    event.preventDefault();
    setNotice('');
    if (!draft.stack && !draft.rcAstro) {
      setNotice('Choose at least one kind of work.');
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
          setNotice(saved.status ? 'Saved. Restart the worker to use the new settings.' : 'Saved.');
        },
      },
    );
  };
  const restart = () => action.mutate(async () => {
    await desktopWorker.stop();
    return desktopWorker.start();
  });
  const chooseFolder = async () => {
    const picked = await tauriFileSystem.pickPath({ title: 'Choose the work folder' });
    if (picked) update({ work_dir: picked });
  };

  if (!loaded.supported) {
    return (
      <div className="settings-section desktop-worker-settings">
        <h3>Run a worker on this computer</h3>
        <p>
          On Linux, run the worker with the <code>psf-guard-worker</code> systemd
          service that the package installs. The Workers guide in the
          documentation explains how to set it up.
        </p>
      </div>
    );
  }

  return (
    <div className="settings-section desktop-worker-settings">
      <UnsavedMark label="worker settings" dirty={dirty} />
      <h3>Run a worker on this computer</h3>
      <p>
        This computer can stack builds and run RC-Astro tools for a PSF Guard
        server. The worker runs in the menu bar on macOS and in the system tray
        on Windows, and keeps running when this window is closed. Create a token
        in the server&apos;s Settings › Workers, then enter it here.
      </p>

      <section className="desktop-worker-status" aria-label="Worker status" data-running={status !== null}>
        {status ? (
          <>
            <p role="status">
              <strong>Running.</strong> {connectionText(status)}
              {status.paused && ' Paused: taking no new tasks.'}
            </p>
            {status.tasks.map((task) => (
              <p key={task.id} className="muted">
                {task.label} · {Math.round(task.fraction * 100)}%
              </p>
            ))}
            {status.tasks.length === 0 && !status.paused && <p className="muted">Waiting for tasks.</p>}
          </>
        ) : (
          <p role="status"><strong>Not running.</strong></p>
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
                {status.paused ? 'Resume taking tasks' : 'Pause taking tasks'}
              </button>
              <button type="button" className="browse-button" disabled={busy} onClick={restart}>
                Restart
              </button>
              <button
                type="button"
                className="cancel-button"
                disabled={busy}
                onClick={() => action.mutate(desktopWorker.stop)}
              >
                Stop
              </button>
            </>
          ) : (
            <button
              type="button"
              className="save-button"
              disabled={busy || dirty || !loaded.settings || !loaded.has_token}
              onClick={() => action.mutate(desktopWorker.start)}
            >
              Start the worker
            </button>
          )}
          <OpenLogsButton onError={setLogError} />
        </div>
        {!status && (!loaded.settings || !loaded.has_token) && (
          <p className="muted">Save the server&apos;s address and a worker token to start the worker.</p>
        )}
        {!status && dirty && loaded.settings && <p className="muted">Save the changes to start the worker.</p>}
      </section>

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
            className="file-path-input"
            type="password"
            autoComplete="off"
            value={draft.token}
            placeholder={loaded.has_token ? 'Saved; enter a new token to replace it' : 'psfw_…'}
            onChange={(event) => update({ token: event.target.value })}
            required={!loaded.has_token}
            disabled={busy}
          />
        </div>
        <div className="database-config">
          <label htmlFor="desktop-worker-name">Worker name</label>
          <input
            id="desktop-worker-name"
            className="file-path-input"
            value={draft.name ?? ''}
            placeholder="This computer's name"
            maxLength={64}
            onChange={(event) => update({ name: event.target.value })}
            disabled={busy}
          />
        </div>
        <fieldset className="desktop-worker-work">
          <legend>Work this computer accepts</legend>
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
            RC-Astro tools, when rc-astro is installed here
          </label>
        </fieldset>
        <div className="database-config">
          <label htmlFor="desktop-worker-slots">Tasks at once</label>
          <input
            id="desktop-worker-slots"
            className="file-path-input"
            type="number"
            min={1}
            max={16}
            value={draft.slots}
            onChange={(event) => update({ slots: Number(event.target.value) })}
            disabled={busy}
          />
          <p className="muted">Each task shares this computer&apos;s cores and memory with the others.</p>
        </div>
        <fieldset className="desktop-worker-files">
          <legend>How task files reach this computer</legend>
          <label>
            <input
              type="radio"
              name="desktop-worker-files"
              checked={draft.files === 'remote'}
              onChange={() => update({ files: 'remote' })}
              disabled={busy}
            />
            Through the server
          </label>
          <label>
            <input
              type="radio"
              name="desktop-worker-files"
              checked={draft.files === 'shared'}
              onChange={() => update({ files: 'shared', maps: draft.maps.length ? draft.maps : [{ server: '', local: '' }] })}
              disabled={busy}
            />
            On storage this computer shares with the server
          </label>
          {draft.files === 'shared' && (
            <div className="desktop-worker-maps">
              <p className="muted">
                For each server folder, enter where this computer sees it. A file
                this computer cannot see goes through the server.
              </p>
              {draft.maps.map((map, index) => (
                <div className="token-secret-row" key={index}>
                  <input
                    className="file-path-input"
                    aria-label={`Server folder ${index + 1}`}
                    placeholder="Server folder, such as /srv/astro"
                    value={map.server}
                    onChange={(event) => update({
                      maps: draft.maps.map((each, at) => at === index ? { ...each, server: event.target.value } : each),
                    })}
                    disabled={busy}
                  />
                  <input
                    className="file-path-input"
                    aria-label={`This computer's folder ${index + 1}`}
                    placeholder="This computer's folder, such as Z:\astro"
                    value={map.local}
                    onChange={(event) => update({
                      maps: draft.maps.map((each, at) => at === index ? { ...each, local: event.target.value } : each),
                    })}
                    disabled={busy}
                  />
                  <button
                    type="button"
                    className="cancel-button"
                    aria-label={`Remove shared folder ${index + 1}`}
                    onClick={() => update({ maps: draft.maps.filter((_, at) => at !== index) })}
                    disabled={busy}
                  >
                    Remove
                  </button>
                </div>
              ))}
              <button
                type="button"
                className="browse-button"
                onClick={() => update({ maps: [...draft.maps, { server: '', local: '' }] })}
                disabled={busy}
              >
                + Add a shared folder
              </button>
            </div>
          )}
        </fieldset>
        <div className="database-config">
          <label htmlFor="desktop-worker-folder">Work folder</label>
          <div className="token-secret-row">
            <input
              id="desktop-worker-folder"
              className="file-path-input"
              value={draft.work_dir ?? ''}
              placeholder="The system temporary folder"
              onChange={(event) => update({ work_dir: event.target.value })}
              disabled={busy}
            />
            <button type="button" className="browse-button" onClick={() => void chooseFolder()} disabled={busy}>
              Choose…
            </button>
          </div>
          <p className="muted">
            Tasks keep their files here while they run, and a mosaic plans its canvas
            here. Choose a disk with room for a few full-size frames.
          </p>
        </div>
        <label className="desktop-worker-login">
          <input
            type="checkbox"
            checked={draft.startAtLogin}
            onChange={(event) => update({ startAtLogin: event.target.checked })}
            disabled={busy}
          />
          Start the worker when I log in
        </label>
        <div className="modal-buttons">
          <button type="submit" className="save-button" disabled={busy || !dirty}>
            {busy ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
      {(notice || action.error || logError) && (
        <p className={action.error || logError ? 'error-text' : 'muted'} role={action.error || logError ? 'alert' : 'status'}>
          {action.error ? errorText(action.error) : logError || notice}
        </p>
      )}
    </div>
  );
}
