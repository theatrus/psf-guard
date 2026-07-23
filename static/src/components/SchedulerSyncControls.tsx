import { useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SchedulerSyncKind, SchedulerSyncResponse } from '../api/types';
import type { DbEntry } from '../utils/tauri';

interface SchedulerSyncControlsProps {
  databases: DbEntry[];
  disabled?: boolean;
}

interface PendingSync {
  kind: SchedulerSyncKind;
  previewId: string;
  expiresAt: number;
  result: SchedulerSyncResponse;
}

const operationLabel = (kind: SchedulerSyncKind) => {
  switch (kind) {
    case 'pull':
      return 'Merge catalogs';
    case 'push_planning':
      return 'Send planning';
    case 'push_grades':
      return 'Send reviewed grades';
  }
};

export default function SchedulerSyncControls({
  databases,
  disabled = false,
}: SchedulerSyncControlsProps) {
  const queryClient = useQueryClient();
  const [sourceId, setSourceId] = useState(databases[0]?.id ?? '');
  const [destinationId, setDestinationId] = useState(databases[1]?.id ?? '');
  const [kind, setKind] = useState<SchedulerSyncKind>('pull');
  const [withImageData, setWithImageData] = useState(true);
  const [pending, setPending] = useState<PendingSync | null>(null);
  const [running, setRunning] = useState(false);
  const [message, setMessage] = useState('');

  const source = useMemo(
    () => databases.find((database) => database.id === sourceId) ?? databases[0],
    [databases, sourceId]
  );
  const destination = useMemo(
    () =>
      databases.find((database) => database.id === destinationId) ??
      databases.find((database) => database.id !== source?.id),
    [databases, destinationId, source?.id]
  );

  const invalidatePreview = () => {
    setPending(null);
    setMessage('');
  };

  const changeSource = (nextSourceId: string) => {
    setSourceId(nextSourceId);
    if (nextSourceId === destination?.id) {
      setDestinationId(source?.id ?? '');
    }
    invalidatePreview();
  };

  const changeDestination = (nextDestinationId: string) => {
    setDestinationId(nextDestinationId);
    if (nextDestinationId === source?.id) {
      setSourceId(destination?.id ?? '');
    }
    invalidatePreview();
  };

  const swapEndpoints = () => {
    if (!source || !destination) return;
    setSourceId(destination.id);
    setDestinationId(source.id);
    invalidatePreview();
  };

  const preview = async () => {
    if (!source || !destination || source.id === destination.id) return;
    setRunning(true);
    setMessage('');
    try {
      const localDbId = kind === 'pull' ? destination.id : source.id;
      const peerDbId = kind === 'pull' ? source.id : destination.id;
      const response = await apiClient.previewDatabaseSync(localDbId, {
        peer_db_id: peerDbId,
        kind,
        dry_run: true,
        with_image_data: withImageData,
        reviewed_only: kind === 'push_grades',
      });
      setPending({
        kind,
        previewId: response.preview_id,
        expiresAt: response.expires_at,
        result: response.result,
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      setMessage(`Preview failed: ${detail}`);
    } finally {
      setRunning(false);
    }
  };

  const apply = async () => {
    if (!pending) return;
    setRunning(true);
    setMessage('');
    try {
      const result = await apiClient.applyDatabaseSyncPreview(
        pending.result.kind === 'pull'
          ? pending.result.destination_db_id
          : pending.result.source_db_id,
        pending.previewId
      );
      setPending(null);
      setMessage(
        `${operationLabel(result.kind)} complete: ${result.total_inserted} added, ` +
          `${result.total_updated} updated.`
      );
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      queryClient.invalidateQueries({ queryKey: ['db'] });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      setMessage(`Apply failed: ${detail}`);
      setPending(null);
    } finally {
      setRunning(false);
    }
  };

  if (databases.length < 2) {
    return (
      <div className="scheduler-sync-empty muted">
        Add a second catalog to merge data or send planning and grades.
      </div>
    );
  }

  return (
    <section className="settings-section scheduler-sync-workspace">
      <div className="scheduler-sync-heading">
        <div>
          <h3>Data Transfer</h3>
          <p>
            Move catalog records in one direction. Preview is always required before
            Apply.
          </p>
        </div>
        <span className="scheduler-sync-safety">Preview first</span>
      </div>

      <div className="scheduler-sync-operation" role="group" aria-label="Transfer operation">
        {(['pull', 'push_planning', 'push_grades'] as SchedulerSyncKind[]).map(
          (operation) => (
            <button
              key={operation}
              type="button"
              className={kind === operation ? 'active' : ''}
              aria-pressed={kind === operation}
              onClick={() => {
                setKind(operation);
                invalidatePreview();
              }}
              disabled={disabled || running}
            >
              {operationLabel(operation)}
            </button>
          )
        )}
      </div>

      <div className="scheduler-sync-endpoints">
        <label>
          <span>Source</span>
          <select
            aria-label="Transfer source"
            value={source?.id ?? ''}
            onChange={(event) => changeSource(event.target.value)}
            disabled={disabled || running}
          >
            {databases.map((database) => (
              <option key={database.id} value={database.id}>
                {database.name}
              </option>
            ))}
          </select>
          <small>{source?.db_path}</small>
        </label>

        <button
          type="button"
          className="scheduler-sync-swap"
          onClick={swapEndpoints}
          disabled={disabled || running}
          aria-label="Swap source and destination"
          title="Swap source and destination"
        >
          ⇄
        </button>

        <label>
          <span>Destination</span>
          <select
            aria-label="Transfer destination"
            value={destination?.id ?? ''}
            onChange={(event) => changeDestination(event.target.value)}
            disabled={disabled || running}
          >
            {databases.map((database) => (
              <option key={database.id} value={database.id}>
                {database.name}
              </option>
            ))}
          </select>
          <small>{destination?.db_path}</small>
        </label>
      </div>

      <div className="scheduler-sync-description">
        {kind === 'pull' && (
          <>
            <strong>Merge projects and captures</strong>
            <span>
              Adds or updates catalog structure and captures. Reviewed grades already at
              the destination win.
            </span>
            <label className="scheduler-sync-check">
              <input
                type="checkbox"
                checked={withImageData}
                onChange={(event) => {
                  setWithImageData(event.target.checked);
                  invalidatePreview();
                }}
                disabled={disabled || running}
              />
              Include stored image thumbnails
            </label>
          </>
        )}
        {kind === 'push_planning' && (
          <>
            <strong>Send projects and exposure plans</strong>
            <span>
              Source planning settings win. Capture counts, images, and grades at the
              destination stay unchanged.
            </span>
          </>
        )}
        {kind === 'push_grades' && (
          <>
            <strong>Send reviewed grades and reject reasons</strong>
            <span>
              Accepted and Rejected rows update matching image GUIDs. Pending rows cannot
              erase a destination decision.
            </span>
          </>
        )}
      </div>

      <div className="scheduler-sync-primary-action">
        <button
          className="browse-button"
          onClick={preview}
          disabled={disabled || running || !source || !destination}
        >
          Preview changes
        </button>
      </div>

      {running && <div className="scheduler-sync-status">Checking databases…</div>}
      {pending && !running && (
        <div className="scheduler-sync-preview">
          {pending.result.grades ? (
            <>
              <strong>
                {pending.result.grades.changed} reviewed grade(s) will change
              </strong>
              <small>
                {pending.result.grades.matched} matched,{' '}
                {pending.result.grades.unchanged} unchanged,{' '}
                {pending.result.grades.unmatched_source} missing at the destination
                {pending.result.grades.duplicate_guids > 0
                  ? `, ${pending.result.grades.duplicate_guids} duplicate GUID(s) skipped`
                  : ''}
              </small>
              {Object.keys(pending.result.grades.transitions).length > 0 && (
                <ul className="scheduler-sync-transitions">
                  {Object.entries(pending.result.grades.transitions).map(([label, count]) => (
                    <li key={label}>
                      {label}: {count}
                    </li>
                  ))}
                </ul>
              )}
            </>
          ) : (
            <>
              <strong>
                {pending.result.total_inserted} rows will be added and{' '}
                {pending.result.total_updated} updated
              </strong>
              <small>
                Projects +{pending.result.project.inserted}/
                {pending.result.project.updated}, targets +
                {pending.result.target.inserted}/{pending.result.target.updated}, plans +
                {pending.result.exposureplan.inserted}/
                {pending.result.exposureplan.updated}
                {pending.result.acquiredimage
                  ? `, captures +${pending.result.acquiredimage.inserted}/${pending.result.acquiredimage.updated}`
                  : ''}
              </small>
            </>
          )}
          <small>
            Preview expires at {new Date(pending.expiresAt * 1000).toLocaleTimeString()}.
            Changing either database makes it stale.
          </small>
          <div className="scheduler-sync-confirm">
            <button className="save-button" onClick={apply} disabled={disabled}>
              Apply this preview
            </button>
            <button className="cancel-button" onClick={() => setPending(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {message && <div className="scheduler-sync-status">{message}</div>}
    </section>
  );
}
