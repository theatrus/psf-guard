import { useEffect, useState, type FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import axios from 'axios';
import { apiClient } from '../api/client';
import type {
  ApiResponse,
  CreatedWorkerToken,
  WorkerSessionSummary,
  WorkersSettings,
} from '../api/types';
import { formatTokenDate } from '../utils/apiTokens';

const WORKERS_QUERY_KEY = ['workers'] as const;

function errorMessage(error: unknown): string {
  if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
    return error.response?.data?.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

/** The address a worker on another machine uses to reach this server. */
function serverAddress(): string {
  const { origin } = window.location;
  return origin.startsWith('http') ? origin : 'http://SERVER:PORT';
}

function lastSeen(worker: WorkerSessionSummary): string {
  if (worker.connected) return 'Connected';
  const minutes = Math.round(worker.seconds_since_seen / 60);
  return minutes < 1 ? 'Not heard from for a minute' : `Not heard from for ${minutes} min`;
}

function fileAccess(worker: WorkerSessionSummary): string {
  if (worker.local) return 'Reads task files in place';
  if (worker.exchange_shared) return 'Shared files';
  if (worker.file_mode === 'shared') {
    return `Files go through the server: ${worker.exchange_problem ?? 'the exchange folder is not visible to the worker'}`;
  }
  return 'Files go through the server';
}

function WorkerRow({ worker }: { worker: WorkerSessionSummary }) {
  return (
    <div className="user-row worker-row" data-worker={worker.name}>
      <div className="user-row-main">
        <div className="user-row-title">
          <strong>{worker.name}</strong>
          {worker.local && <span className="user-source-badge">This server</span>}
          <span className="worker-state" data-connected={worker.connected}>
            {lastSeen(worker)}
          </span>
        </div>
        <div className="user-row-details">
          <span className="muted">
            {worker.rc_astro.length
              ? worker.rc_astro.join(', ')
              : 'No RC-Astro tools'}
          </span>
          <span className="muted">{fileAccess(worker)}</span>
          {!worker.local && (
            <span className="muted">
              PSF Guard {worker.version} · {worker.threads} threads
            </span>
          )}
        </div>
        {worker.running && (
          <div className="user-row-details">
            <span>
              Running {worker.running.label} · {Math.round(worker.running.fraction * 100)}%
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Settings › Workers: the machines that may run tasks for this server, the
 * tokens they join with, and the folder their task files go through.
 */
export default function WorkerSettings() {
  const queryClient = useQueryClient();
  const workers = useQuery({
    queryKey: WORKERS_QUERY_KEY,
    queryFn: apiClient.getWorkers,
    refetchInterval: 5000,
  });
  const [name, setName] = useState('');
  const [formOpen, setFormOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState('');
  const [created, setCreated] = useState<CreatedWorkerToken | null>(null);
  const [copied, setCopied] = useState(false);
  const savedFolder = workers.data?.exchange_folder_setting ?? '';
  const [folder, setFolder] = useState(savedFolder);
  useEffect(() => setFolder(savedFolder), [savedFolder]);

  const publish = (settings: WorkersSettings) => {
    queryClient.setQueryData(WORKERS_QUERY_KEY, settings);
  };

  const run = async (action: () => Promise<void>) => {
    setSaving(true);
    setStatus('');
    try {
      await action();
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const create = (event: FormEvent) => {
    event.preventDefault();
    void run(async () => {
      const result = await apiClient.createWorkerToken(name.trim());
      publish(result.workers);
      setCreated(result);
      setCopied(false);
      setFormOpen(false);
      setName('');
    });
  };

  const revoke = (id: string, tokenName: string) => {
    if (!window.confirm(`Revoke the token for ${tokenName}? The worker disconnects, and its task goes back in the queue.`)) {
      return;
    }
    void run(async () => {
      publish(await apiClient.revokeWorkerToken(id));
      if (created?.summary.id === id) setCreated(null);
    });
  };

  const saveFolder = (next: string | null) => {
    void run(async () => {
      publish(await apiClient.setWorkerExchangeFolder(next));
      setStatus(next ? 'Exchange folder saved.' : 'The exchange folder is back to the default.');
    });
  };

  const command = created
    ? `PSF_GUARD_WORKER_TOKEN=${created.token} psf-guard worker --server ${serverAddress()}`
    : '';

  const copyCommand = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
    } catch {
      setStatus('Copy failed; select the command and copy it by hand.');
    }
  };

  if (workers.isLoading) return <div className="detecting-database">Loading workers…</div>;
  if (workers.isError || !workers.data) {
    return (
      <div className="settings-section">
        <h3>Workers</h3>
        <p className="status-message error" role="alert">{errorMessage(workers.error)}</p>
      </div>
    );
  }
  const data = workers.data;
  const connectedTokens = new Set(
    data.workers.filter((worker) => worker.connected).map((worker) => worker.token_id),
  );

  return (
    <div className="settings-section user-management worker-settings">
      <div className="user-management-heading">
        <div>
          <h3>Workers</h3>
          <p>
            A worker is another computer that runs RC-Astro tools for this
            server, such as one with a GPU or an RC-Astro licence. Install PSF
            Guard there, create a token here, and run{' '}
            <code>psf-guard worker</code> with it. A connected worker runs
            the tools it takes ahead of this server. This server runs a tool
            itself, when rc-astro is installed here, while no worker that
            takes it is connected.
          </p>
        </div>
        {!formOpen && (
          <div className="db-row-actions">
            <button
              type="button"
              className="add-directory-button"
              onClick={() => {
                setFormOpen(true);
                setCreated(null);
                setStatus('');
              }}
            >
              + Add a worker
            </button>
          </div>
        )}
      </div>

      {formOpen && (
        <form className="user-form" onSubmit={create}>
          <div className="database-config">
            <label htmlFor="worker-name">Worker name</label>
            <input
              id="worker-name"
              className="file-path-input"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="studio-pc"
              maxLength={64}
              required
              disabled={saving}
            />
          </div>
          <div className="modal-buttons">
            <button type="submit" className="save-button" disabled={saving}>
              {saving ? 'Creating…' : 'Create token'}
            </button>
            <button type="button" className="cancel-button" disabled={saving} onClick={() => setFormOpen(false)}>
              Cancel
            </button>
          </div>
        </form>
      )}

      {created && (
        <div className="token-secret" role="status">
          <p>
            <strong>Copy this command now.</strong> The token in it is not shown
            again. Run it on {created.summary.name}:
          </p>
          <div className="token-secret-row">
            <code className="token-secret-value">{command}</code>
            <button type="button" className="browse-button" onClick={() => void copyCommand()}>
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <p className="muted">
            Add <code>--files shared --map SERVER_FOLDER=LOCAL_FOLDER</code> when
            the worker sees this server&apos;s exchange folder on shared storage.
            Otherwise every file goes through the server.
          </p>
          <button type="button" className="cancel-button" onClick={() => setCreated(null)}>
            Done
          </button>
        </div>
      )}

      <h4>Connected</h4>
      {data.workers.map((worker) => (
        <WorkerRow key={worker.session} worker={worker} />
      ))}
      {data.queued.length > 0 && (
        <>
          <h4>Waiting for a worker</h4>
          <ul className="worker-queue">
            {data.queued.map((task, index) => (
              <li key={index}>
                {task.label} <span className="muted">needs {task.needs}</span>
              </li>
            ))}
          </ul>
        </>
      )}

      <h4>Tokens</h4>
      {data.tokens.length === 0 && <p className="muted">No worker tokens yet.</p>}
      {data.tokens.map((token) => (
        <div className="user-row token-row" key={token.id}>
          <div className="user-row-main">
            <div className="user-row-title">
              <strong>{token.name}</strong>
            </div>
            <div className="user-row-details">
              <span className="muted">Created {formatTokenDate(token.created_at)}</span>
              <span className="muted">
                {connectedTokens.has(token.id) ? 'In use' : 'Not connected'}
              </span>
            </div>
          </div>
          <div className="db-row-actions">
            <button
              type="button"
              className="remove-button"
              disabled={saving}
              aria-label={`Revoke the token for ${token.name}`}
              onClick={() => revoke(token.id, token.name)}
            >
              Revoke
            </button>
          </div>
        </div>
      ))}

      <h4>Exchange folder</h4>
      <p className="muted">
        Task files go through this folder: the image a tool processes and the
        result it returns. A worker in shared mode reads and writes them here
        directly, so choose a folder on storage the workers can reach. Now:{' '}
        <code>{data.exchange_folder}</code>
      </p>
      <form
        className="database-config"
        onSubmit={(event) => {
          event.preventDefault();
          saveFolder(folder.trim() || null);
        }}
      >
        <label htmlFor="worker-exchange-folder">Folder</label>
        <input
          id="worker-exchange-folder"
          className="file-path-input"
          value={folder}
          onChange={(event) => setFolder(event.target.value)}
          placeholder={data.default_exchange_folder}
          disabled={saving}
        />
        <div className="modal-buttons">
          <button type="submit" className="save-button" disabled={saving || folder.trim() === savedFolder}>
            Save folder
          </button>
          <button
            type="button"
            className="cancel-button"
            disabled={saving || !savedFolder}
            onClick={() => saveFolder(null)}
          >
            Use the default
          </button>
        </div>
      </form>

      {status && <div className="status-message">{status}</div>}
    </div>
  );
}
