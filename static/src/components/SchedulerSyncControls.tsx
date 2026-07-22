import { useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SchedulerSyncKind, SchedulerSyncResponse } from '../api/types';
import type { DbEntry } from '../utils/tauri';

interface SchedulerSyncControlsProps {
  database: DbEntry;
  databases: DbEntry[];
  disabled?: boolean;
}

interface PendingSync {
  kind: SchedulerSyncKind;
  result: SchedulerSyncResponse;
}

const actionLabel = (kind: SchedulerSyncKind) =>
  kind === 'pull' ? 'full pull' : 'planning push';

export default function SchedulerSyncControls({
  database,
  databases,
  disabled = false,
}: SchedulerSyncControlsProps) {
  const queryClient = useQueryClient();
  const peers = useMemo(
    () => databases.filter((candidate) => candidate.id !== database.id),
    [database.id, databases]
  );
  const [peerId, setPeerId] = useState(peers[0]?.id ?? '');
  const [withImageData, setWithImageData] = useState(true);
  const [pending, setPending] = useState<PendingSync | null>(null);
  const [running, setRunning] = useState(false);
  const [message, setMessage] = useState('');

  const selectedPeerId = peers.some((peer) => peer.id === peerId)
    ? peerId
    : (peers[0]?.id ?? '');
  const peerName =
    peers.find((peer) => peer.id === selectedPeerId)?.name ?? selectedPeerId;

  const run = async (kind: SchedulerSyncKind, dryRun: boolean) => {
    if (!selectedPeerId) return;
    setRunning(true);
    setMessage('');
    try {
      const result = await apiClient.syncDatabase(database.id, {
        peer_db_id: selectedPeerId,
        kind,
        dry_run: dryRun,
        with_image_data: withImageData,
      });
      if (dryRun) {
        setPending({ kind, result });
      } else {
        setPending(null);
        setMessage(
          `${kind === 'pull' ? 'Pulled from' : 'Pushed planning settings to'} ${peerName}: ` +
            `${result.total_inserted} added, ${result.total_updated} updated.`
        );
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db'] });
      }
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      setMessage(`${dryRun ? 'Preview' : 'Sync'} failed: ${detail}`);
    } finally {
      setRunning(false);
    }
  };

  if (peers.length === 0) {
    return (
      <div className="scheduler-sync-empty muted">
        Add the telescope scheduler database here to sync projects and plans.
      </div>
    );
  }

  return (
    <details className="scheduler-sync-controls">
      <summary>Scheduler database sync</summary>
      <div className="scheduler-sync-body">
        <label>
          Other database
          <select
            value={selectedPeerId}
            onChange={(event) => {
              setPeerId(event.target.value);
              setPending(null);
              setMessage('');
            }}
            disabled={disabled || running}
          >
            {peers.map((peer) => (
              <option value={peer.id} key={peer.id}>
                {peer.name}
              </option>
            ))}
          </select>
        </label>

        <div className="scheduler-sync-actions">
          <div>
            <strong>Pull full projects into {database.name}</strong>
            <small>
              Adds or updates projects, targets, plans, captures, and grades from {peerName}.
              Local reviewed grades win.
            </small>
            <label className="scheduler-sync-check">
              <input
                type="checkbox"
                checked={withImageData}
                onChange={(event) => {
                  setWithImageData(event.target.checked);
                  setPending(null);
                  setMessage('');
                }}
                disabled={disabled || running}
              />
              Include stored image thumbnails
            </label>
            <button
              className="browse-button"
              onClick={() => run('pull', true)}
              disabled={disabled || running}
            >
              Preview full pull
            </button>
          </div>

          <div>
            <strong>Push planning settings to {peerName}</strong>
            <small>
              Adds or updates projects, targets, templates, plans, and rule weights. Capture
              counts, images, and grades on {peerName} stay unchanged.
            </small>
            <button
              className="browse-button"
              onClick={() => run('push_planning', true)}
              disabled={disabled || running}
            >
              Preview planning push
            </button>
          </div>
        </div>

        {running && <div className="scheduler-sync-status">Checking databases…</div>}
        {pending && !running && (
          <div className="scheduler-sync-preview">
            <strong>
              Preview: {pending.result.total_inserted} rows added and{' '}
              {pending.result.total_updated} updated
            </strong>
            <small>
              Projects +{pending.result.project.inserted}/{pending.result.project.updated}, targets
              +{pending.result.target.inserted}/{pending.result.target.updated}, plans +
              {pending.result.exposureplan.inserted}/{pending.result.exposureplan.updated}
              {pending.result.acquiredimage
                ? `, captures +${pending.result.acquiredimage.inserted}/${pending.result.acquiredimage.updated}`
                : ''}
            </small>
            <div className="scheduler-sync-confirm">
              <button
                className="save-button"
                onClick={() => run(pending.kind, false)}
                disabled={disabled}
              >
                Apply {actionLabel(pending.kind)}
              </button>
              <button className="cancel-button" onClick={() => setPending(null)}>
                Cancel
              </button>
            </div>
          </div>
        )}
        {message && <div className="scheduler-sync-status">{message}</div>}
      </div>
    </details>
  );
}
