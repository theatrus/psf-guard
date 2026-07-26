import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { apiClient } from '../api/client';
import type { PeerCheck, RemoteSyncDirection, RemoteSyncResult } from '../api/types';
import type { DbEntry } from '../utils/tauri';

interface RemotePeerSyncProps {
  databases: DbEntry[];
  disabled?: boolean;
}

const DIRECTIONS: Array<{ value: RemoteSyncDirection; label: string; note: string }> = [
  {
    value: 'pull',
    label: 'Pull from peer',
    note: 'Brings the peer’s projects, targets, and captures here. Grades reviewed here are kept.',
  },
  {
    value: 'push_planning',
    label: 'Send planning',
    note: 'Sends projects, targets, templates, and plans. Capture history and grades there are untouched.',
  },
  {
    value: 'push_grades',
    label: 'Send grades',
    note: 'Sends grading decisions and reject reasons, matched by image GUID.',
  },
];

/** Counts worth showing: a merge reports dozens, nearly all of them zero. */
function summaryLine(summary: Record<string, number>): string {
  const shown = Object.entries(summary)
    .filter(([, count]) => count !== 0)
    .map(([name, count]) => `${name.replace(/_/g, ' ')} ${count}`);
  return shown.length ? shown.join(', ') : 'nothing to change';
}

export default function RemotePeerSync({ databases, disabled }: RemotePeerSyncProps) {
  const queryClient = useQueryClient();
  const peers = useQuery({
    queryKey: ['peers'],
    queryFn: () => apiClient.getPeers(),
    staleTime: 60_000,
  });

  const [peerId, setPeerId] = useState('');
  const [localDbId, setLocalDbId] = useState('');
  const [direction, setDirection] = useState<RemoteSyncDirection>('pull');
  const [allGrades, setAllGrades] = useState(false);
  const [check, setCheck] = useState<PeerCheck | null>(null);
  const [preview, setPreview] = useState<RemoteSyncResult | null>(null);
  const [message, setMessage] = useState('');

  const [adding, setAdding] = useState(false);
  const [formName, setFormName] = useState('');
  const [formUrl, setFormUrl] = useState('https://');
  const [formToken, setFormToken] = useState('');

  const peerList = peers.data ?? [];
  const selectedPeer = peerList.find((peer) => peer.id === peerId) ?? peerList[0];
  const selectedDb =
    databases.find((database) => database.id === localDbId) ?? databases[0];
  const busy = disabled || peers.isLoading;

  /** Any change to what would be sent invalidates a preview taken before it. */
  const invalidatePreview = () => {
    setPreview(null);
    setMessage('');
  };

  const addPeer = useMutation({
    mutationFn: () =>
      apiClient.addPeer({
        name: formName.trim(),
        base_url: formUrl.trim(),
        token: formToken.trim(),
      }),
    onSuccess: (peer) => {
      setAdding(false);
      setFormName('');
      setFormUrl('https://');
      setFormToken('');
      setPeerId(peer.id);
      setMessage(`Added ${peer.name}.`);
      queryClient.invalidateQueries({ queryKey: ['peers'] });
    },
    onError: (error: Error) => setMessage(`Could not add the peer: ${error.message}`),
  });

  const removePeer = useMutation({
    mutationFn: (id: string) => apiClient.removePeer(id),
    onSuccess: () => {
      setPeerId('');
      setCheck(null);
      invalidatePreview();
      queryClient.invalidateQueries({ queryKey: ['peers'] });
    },
    onError: (error: Error) => setMessage(`Could not remove the peer: ${error.message}`),
  });

  const checkPeer = useMutation({
    mutationFn: (id: string) => apiClient.checkPeer(id),
    onSuccess: (result) => {
      setCheck(result);
      setMessage(
        result.reachable
          ? `${result.product ?? 'Peer'} ${result.product_version ?? ''} — catalog ${
              result.catalogs[0] ?? 'unknown'
            }`.trim()
          : `Could not reach the peer: ${result.error ?? 'no reason given'}`
      );
    },
    onError: (error: Error) => setMessage(`Could not reach the peer: ${error.message}`),
  });

  const run = useMutation({
    mutationFn: ({ dryRun }: { dryRun: boolean }) =>
      apiClient.syncWithPeer(selectedDb!.id, {
        peer_id: selectedPeer!.id,
        direction,
        dry_run: dryRun,
        reviewed_only: !allGrades,
        with_image_data: true,
      }),
    onSuccess: (result) => {
      if (result.applied) {
        setPreview(null);
        setMessage(`Applied: ${summaryLine(result.summary)}.`);
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db'] });
      } else {
        setPreview(result);
        setMessage('');
      }
    },
    onError: (error: Error) => {
      // A refused apply wrote nothing on either side; keep the preview so the
      // operator can read what it said and decide.
      setMessage(`Sync failed: ${error.message}`);
    },
  });

  if (databases.length === 0) {
    return (
      <div className="scheduler-sync-empty muted">
        Add a catalog before syncing with a remote PSF Guard.
      </div>
    );
  }

  return (
    <section className="settings-section remote-peer-sync">
      <div className="scheduler-sync-heading">
        <div>
          <h3>Remote PSF Guard</h3>
          <p>
            Sync with another PSF Guard over the network. Preview is always required
            before Apply, and the API key stays on this server.
          </p>
        </div>
        <span className="scheduler-sync-safety">Preview first</span>
      </div>

      <div className="remote-peer-list">
        <label>
          <span>Peer</span>
          <select
            aria-label="Remote peer"
            value={selectedPeer?.id ?? ''}
            onChange={(event) => {
              setPeerId(event.target.value);
              setCheck(null);
              invalidatePreview();
            }}
            disabled={busy || peerList.length === 0}
          >
            {peerList.length === 0 && <option value="">No peers configured</option>}
            {peerList.map((peer) => (
              <option key={peer.id} value={peer.id}>
                {peer.name}
              </option>
            ))}
          </select>
          {selectedPeer && <small>{selectedPeer.base_url}</small>}
        </label>

        <div className="remote-peer-actions">
          <button
            type="button"
            onClick={() => selectedPeer && checkPeer.mutate(selectedPeer.id)}
            disabled={busy || !selectedPeer || checkPeer.isPending}
          >
            {checkPeer.isPending ? 'Checking…' : 'Test connection'}
          </button>
          <button type="button" onClick={() => setAdding((open) => !open)} disabled={busy}>
            {adding ? 'Cancel' : 'Add peer'}
          </button>
          {selectedPeer && (
            <button
              type="button"
              className="danger"
              onClick={() => removePeer.mutate(selectedPeer.id)}
              disabled={busy || removePeer.isPending}
            >
              Remove
            </button>
          )}
        </div>
      </div>

      {adding && (
        <div className="remote-peer-form">
          <label>
            <span>Name</span>
            <input
              value={formName}
              onChange={(event) => setFormName(event.target.value)}
              placeholder="Telescope"
            />
          </label>
          <label>
            <span>Base URL</span>
            <input
              value={formUrl}
              onChange={(event) => setFormUrl(event.target.value)}
              placeholder="https://telescope.example:3000"
            />
          </label>
          <label>
            <span>API key</span>
            <input
              type="password"
              value={formToken}
              onChange={(event) => setFormToken(event.target.value)}
              placeholder="The key that peer issued for its catalog"
            />
          </label>
          {/* Outside the label: help text inside one becomes part of the
              field's accessible name. */}
          <small>
            The key is stored on this server and sent to the peer on every request. It
            is never returned to this page.
          </small>
          <button
            type="button"
            className="save-button"
            onClick={() => addPeer.mutate()}
            disabled={
              addPeer.isPending ||
              !formName.trim() ||
              !formToken.trim() ||
              !/^https?:\/\/.+/.test(formUrl.trim())
            }
          >
            {addPeer.isPending ? 'Adding…' : 'Save peer'}
          </button>
        </div>
      )}

      {check?.reachable && (
        <p className="remote-peer-check muted">
          Speaks protocol {check.protocol_version}; offers{' '}
          {check.capabilities.join(', ') || 'nothing'}.
        </p>
      )}

      <div className="scheduler-sync-operation" role="group" aria-label="Sync direction">
        {DIRECTIONS.map((option) => (
          <button
            key={option.value}
            type="button"
            className={direction === option.value ? 'active' : ''}
            aria-pressed={direction === option.value}
            onClick={() => {
              setDirection(option.value);
              invalidatePreview();
            }}
            disabled={busy}
          >
            {option.label}
          </button>
        ))}
      </div>
      <p className="muted">{DIRECTIONS.find((o) => o.value === direction)?.note}</p>

      <label className="remote-peer-local">
        <span>Local catalog</span>
        <select
          aria-label="Local catalog"
          value={selectedDb?.id ?? ''}
          onChange={(event) => {
            setLocalDbId(event.target.value);
            invalidatePreview();
          }}
          disabled={busy}
        >
          {databases.map((database) => (
            <option key={database.id} value={database.id}>
              {database.name}
            </option>
          ))}
        </select>
      </label>

      {direction === 'push_grades' && (
        <label className="remote-peer-toggle">
          <input
            type="checkbox"
            checked={allGrades}
            onChange={(event) => {
              setAllGrades(event.target.checked);
              invalidatePreview();
            }}
            disabled={busy}
          />
          <span>
            Send every grade, not only reviewed ones. Off by default, so a Pending row
            here cannot erase a decision there.
          </span>
        </label>
      )}

      <div className="remote-peer-run">
        <button
          type="button"
          onClick={() => run.mutate({ dryRun: true })}
          disabled={busy || !selectedPeer || !selectedDb || run.isPending}
        >
          {run.isPending ? 'Working…' : 'Preview changes'}
        </button>
        {preview && (
          <button
            type="button"
            className="save-button"
            onClick={() => run.mutate({ dryRun: false })}
            disabled={run.isPending}
          >
            Apply this preview
          </button>
        )}
      </div>

      {preview && (
        <div className="remote-peer-preview" role="status">
          <strong>{preview.peer_product}</strong> — catalog {preview.peer_catalog}
          <p>{summaryLine(preview.summary)}</p>
        </div>
      )}
      {message && <p className="remote-peer-message">{message}</p>}
    </section>
  );
}
