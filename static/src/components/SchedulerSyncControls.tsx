import { useEffect, useMemo, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SchedulerSyncKind, SchedulerSyncResponse } from '../api/types';
import type { DbEntry } from '../utils/tauri';

interface SchedulerSyncControlsProps {
  databases: DbEntry[];
  disabled?: boolean;
}

interface PendingSync {
  localDbId: string;
  previewId: string;
  expiresAt: number;
  result: SchedulerSyncResponse;
}

const STORED_PREVIEW_KEY = 'psf-guard.scheduler-sync-preview';

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

const plural = (count: number, noun: string) => `${count} ${noun}${count === 1 ? '' : 's'}`;

/** Projects the source catalog deleted that the transfer deletes here
 *  too, because nothing was captured for them here. */
const deletedProjectsLine = (result: SchedulerSyncResponse) => {
  const projects = result.project.deleted ?? 0;
  if (projects === 0) return null;
  const parts = [
    (result.target.deleted ?? 0) > 0 ? plural(result.target.deleted ?? 0, 'target') : null,
    (result.exposureplan.deleted ?? 0) > 0 ? plural(result.exposureplan.deleted ?? 0, 'exposure plan') : null,
  ].filter((part): part is string => part !== null);
  return `Deletes ${plural(projects, 'project')} that the source catalog deleted${
    parts.length > 0 ? `, with ${parts.join(' and ')}` : ''
  }. Nothing was captured for ${projects === 1 ? 'it' : 'them'} in the destination.`;
};

/** The Keep out and Use anyway choices a pull carries: each changes which
 * frames the destination's masters take, so the preview says so. */
const calibrationChoiceLine = (result: SchedulerSyncResponse) => {
  const calibration = result.calibration;
  if (!calibration) return null;
  const parts = [
    calibration.kept_out > 0 ? `${calibration.kept_out} kept out` : null,
    calibration.used_anyway > 0 ? `${calibration.used_anyway} used anyway` : null,
    calibration.cleared > 0 ? `${calibration.cleared} back to their grade` : null,
  ].filter((part): part is string => part !== null);
  const frames = parts.length > 0;
  // A person's splits and joins of flat groups, and lights' flat sides.
  if ((calibration.flat_groups ?? 0) > 0) parts.push(`${calibration.flat_groups} flat group split(s) or join(s)`);
  if ((calibration.flat_lights ?? 0) > 0) parts.push(`${calibration.flat_lights} light flat choice(s)`);
  if (parts.length === 0) return null;
  return `Calibration choices: ${parts.join(', ')}. ${frames
    ? 'Masters built from these frames change.'
    : 'The flats used to calibrate these lights may change.'}`;
};

export default function SchedulerSyncControls({
  databases: catalogs,
  disabled = false,
}: SchedulerSyncControlsProps) {
  const databases = useMemo(() => catalogs.flatMap(catalog => [catalog, ...(catalog.ts_sync ? [{
    ...catalog, id: `ts:${catalog.id}`, name: `${catalog.name} — Target Scheduler`,
    db_path: catalog.ts_sync.source_path, ts_sync: undefined,
  }] : [])]), [catalogs]);
  const queryClient = useQueryClient();
  const firstTs = databases.find(database => database.id.startsWith('ts:'));
  const [sourceId, setSourceId] = useState(firstTs?.id ?? databases[0]?.id ?? '');
  const [destinationId, setDestinationId] = useState(firstTs?.id.slice(3) ?? databases[1]?.id ?? '');
  const [kind, setKind] = useState<SchedulerSyncKind>('pull');
  const [withImageData, setWithImageData] = useState(true);
  const [pending, setPending] = useState<PendingSync | null>(null);
  // A preview runs while the inputs stay live: a change stops it, and a new
  // Preview replaces it. Only Apply locks the form.
  const [previewingDbId, setPreviewingDbId] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);
  /** Counts preview requests, so a replaced request's reply is ignored. */
  const latestPreview = useRef(0);
  /** The stop sent for a replaced preview. The next preview waits for it,
   *  so a late stop never reaches the new one. */
  const pendingStop = useRef<Promise<unknown>>(Promise.resolve());
  const running = applying || previewingDbId !== null;
  const [message, setMessage] = useState('');
  const recoveryAttempted = useRef(false);

  const source = useMemo(
    () => databases.find((database) => database.id === sourceId) ?? databases.find(database => database.id.startsWith('ts:')) ?? databases[0],
    [databases, sourceId]
  );
  const destination = useMemo(
    () =>
      databases.find((database) => database.id === destinationId) ??
      databases.find((database) => source?.id === `ts:${database.id}`) ??
      databases.find((database) => database.id !== source?.id),
    [databases, destinationId, source?.id]
  );

  const forgetPending = (discardServerPreview: boolean) => {
    if (pending && discardServerPreview) {
      void apiClient
        .deleteDatabaseSyncPreview(pending.localDbId, pending.previewId)
        .catch(() => undefined);
    }
    sessionStorage.removeItem(STORED_PREVIEW_KEY);
    setPending(null);
    setMessage('');
  };

  const invalidatePreview = () => {
    forgetPending(true);
    if (previewingDbId !== null) {
      // The inputs changed: the preview under way answers a question no one
      // asks any more.
      latestPreview.current += 1;
      pendingStop.current = apiClient.stopPreview('sync', previewingDbId).catch(() => undefined);
      setPreviewingDbId(null);
    }
  };

  useEffect(() => {
    if (recoveryAttempted.current || databases.length === 0) return;
    recoveryAttempted.current = true;
    const stored = sessionStorage.getItem(STORED_PREVIEW_KEY);
    if (!stored) return;

    let cancelled = false;
    void (async () => {
      try {
        const reference = JSON.parse(stored) as {
          localDbId?: string;
          previewId?: string;
        };
        if (!reference.localDbId || !reference.previewId) {
          throw new Error('invalid stored preview');
        }
        const response = await apiClient.getDatabaseSyncPreview(
          reference.localDbId,
          reference.previewId
        );
        if (cancelled) return;
        const restored = {
          localDbId: reference.localDbId,
          previewId: response.preview_id,
          expiresAt: response.expires_at,
          result: response.result,
        };
        setPending(restored);
        setKind(response.result.kind);
        setSourceId(response.result.source_db_id);
        setDestinationId(response.result.destination_db_id);
        setWithImageData(response.result.imagedata !== null);
        setMessage('Restored the pending transfer preview.');
      } catch {
        sessionStorage.removeItem(STORED_PREVIEW_KEY);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [databases]);

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

  const invalidDirection = kind === 'pull' ? destination?.id.startsWith('ts:') : source?.id.startsWith('ts:');
  const preview = async () => {
    if (!source || !destination || source.id === destination.id || invalidDirection) return;
    const request = ++latestPreview.current;
    const localDbId = kind === 'pull' ? destination.id : source.id;
    const peerDbId = kind === 'pull' ? source.id : destination.id;
    setPreviewingDbId(localDbId);
    setMessage('');
    try {
      if (pending) {
        await apiClient
          .deleteDatabaseSyncPreview(pending.localDbId, pending.previewId)
          .catch(() => undefined);
        sessionStorage.removeItem(STORED_PREVIEW_KEY);
        setPending(null);
      }
      await pendingStop.current;
      if (request !== latestPreview.current) return;
      const response = await apiClient.previewDatabaseSync(localDbId, {
        peer_db_id: peerDbId,
        kind,
        dry_run: true,
        with_image_data: withImageData,
        reviewed_only: kind === 'push_grades',
      });
      if (request !== latestPreview.current) {
        // Replaced, but finished before its stop arrived: the server kept
        // it, and no one will apply it.
        void apiClient
          .deleteDatabaseSyncPreview(localDbId, response.preview_id)
          .catch(() => undefined);
        return;
      }
      setPending({
        localDbId,
        previewId: response.preview_id,
        expiresAt: response.expires_at,
        result: response.result,
      });
      sessionStorage.setItem(
        STORED_PREVIEW_KEY,
        JSON.stringify({ localDbId, previewId: response.preview_id })
      );
    } catch (error) {
      // A newer preview replaced this one: its failure is not news.
      if (request !== latestPreview.current) return;
      const detail = error instanceof Error ? error.message : String(error);
      setMessage(`Preview failed: ${detail}`);
    } finally {
      if (request === latestPreview.current) setPreviewingDbId(null);
    }
  };

  const apply = async () => {
    if (!pending) return;
    setApplying(true);
    setMessage('');
    try {
      const result = await apiClient.applyDatabaseSyncPreview(
        pending.localDbId,
        pending.previewId
      );
      sessionStorage.removeItem(STORED_PREVIEW_KEY);
      setPending(null);
      const deleted = result.project?.deleted ?? 0;
      setMessage(
        `${operationLabel(result.kind)} complete: ${result.total_inserted} added, ` +
          `${result.total_updated} updated` +
          (deleted > 0 ? `, ${plural(deleted, 'project')} deleted.` : '.')
      );
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      queryClient.invalidateQueries({ queryKey: ['db'] });
    } catch (error) {
      // A refused apply wrote nothing and the server kept the preview, so
      // keep it here too: retrying works for a transient failure, and a
      // stale destination just needs a new preview, which is still offered.
      const detail = error instanceof Error ? error.message : String(error);
      setMessage(`Apply failed: ${detail}`);
    } finally {
      setApplying(false);
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
              aria-pressed={kind === operation}
              onClick={() => {
                if ((operation !== 'pull' && source?.id.startsWith('ts:')) || (operation === 'pull' && destination?.id.startsWith('ts:'))) {
                  if (source && destination) { setSourceId(destination.id); setDestinationId(source.id); }
                }
                setKind(operation);
                invalidatePreview();
              }}
              disabled={disabled || applying}
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
            disabled={disabled || applying}
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
          disabled={disabled || applying}
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
            disabled={disabled || applying}
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

      {invalidDirection && <p role="alert">Import captures into a PSF Guard catalog. Send planning or grades from a PSF Guard catalog to Target Scheduler.</p>}
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
                disabled={disabled || applying}
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
          disabled={disabled || applying || !source || !destination || invalidDirection}
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
              {deletedProjectsLine(pending.result) && (
                <small>{deletedProjectsLine(pending.result)}</small>
              )}
              {calibrationChoiceLine(pending.result) && (
                <small>{calibrationChoiceLine(pending.result)}</small>
              )}
            </>
          )}
          <small>
            Preview expires at {new Date(pending.expiresAt * 1000).toLocaleTimeString()}.
            Changes to the source appear in a new preview. Changes to the destination invalidate this preview.
          </small>
          <div className="scheduler-sync-confirm">
            <button className="save-button" onClick={apply} disabled={disabled}>
              Apply this preview
            </button>
            <button className="cancel-button" onClick={() => forgetPending(true)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {message && <div className="scheduler-sync-status">{message}</div>}
    </section>
  );
}
