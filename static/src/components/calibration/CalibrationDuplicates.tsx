import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CalibrationDuplicateReport,
  CalibrationDuplicateStatus,
  CalibrationPlanChange,
  CalibrationTwinGroup,
} from '../../api/types';
import Dialog from '../Dialog';
import { KIND_LABELS, formatTime, plural, serverTime } from './calibrationLabels';
import { useRigClocks } from './useRigClocks';
import {
  duplicateJobLabel,
  duplicateJobPercent,
  duplicateJobTitle,
  duplicatesKey,
  duplicatesPollMs,
} from './duplicateJob';
import { ALL_RIGS, type CalibrationFilterState } from './useCalibrationUrlState';

interface CalibrationDuplicatesProps {
  dbId: string;
  /** Whether this account may change the library and the server allows
   *  database management. */
  canRepair: boolean;
  /** Why it may not, when it may not. */
  repairNote: string | null;
  /** Whether this account may stop a run: an editor's. */
  canStop: boolean;
  filters: CalibrationFilterState;
}

const shortUuid = (uuid: string) => uuid.slice(0, 8);

function groupNote(group: CalibrationTwinGroup): string {
  const missing = group.members.filter((member) => !member.exists).length;
  const copies = group.members.filter((member) => member.exists).length;
  const parts = [];
  if (missing > 0) parts.push(`${plural(missing, 'row')} whose file is gone`);
  if (copies > 1) parts.push(`${copies} copies on disk`);
  return parts.join(' · ');
}

/** What a repair would do, in the words the preview and the summary share. */
function planLines(report: CalibrationDuplicateReport): string[] {
  const doubled = report.masters.filter((master) => master.doubled).length;
  return [
    `${plural(report.groups.length, 'exposure')} recorded more than once`,
    `${plural(report.rows_removed, 'row')} merged into the row kept: ${report.missing_rows} whose file is gone, ${report.physical_rows} ${report.physical_rows === 1 ? 'second copy' : 'second copies'} on disk`,
    ...(report.repointed > 0 ? [`${plural(report.repointed, 'kept row')} ${report.repointed === 1 ? 'moves' : 'move'} to a live copy, its own file being gone`] : []),
    report.masters.length > 0
      ? `${plural(report.masters.length, 'master')} built again${doubled > 0 ? `, ${doubled} of them averaged an exposure twice` : ''}`
      : 'No master needs building again',
  ];
}

/** A list of file names, with how many more the server left out. */
function names(list: string[], total: number): string {
  const more = total - list.length;
  return list.join(', ') + (more > 0 ? `, and ${more} more` : '');
}

/** What changed between the preview and Apply, in words. */
function changeLines(change: CalibrationPlanChange): string[] {
  const lines = [];
  if (change.added_count > 0) lines.push(`${plural(change.added_count, 'exposure')} newly recorded twice: ${names(change.added, change.added_count)}`);
  if (change.dropped_count > 0) lines.push(`${plural(change.dropped_count, 'exposure')} no longer to repair: ${names(change.dropped, change.dropped_count)}`);
  if (change.altered_count > 0) lines.push(`${plural(change.altered_count, 'exposure')} whose rows or files changed: ${names(change.altered, change.altered_count)}`);
  if (lines.length === 0) lines.push('The rows the preview read changed, though the exposures to repair are the same.');
  return lines;
}

/** "just now", "12 min ago", "3 h ago", or the date. */
function checkedAgo(at: number, now = Date.now() / 1000): string {
  const minutes = Math.floor(Math.max(0, now - at) / 60);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return serverTime(at);
}

/** A check or repair at work: what it does, how far, and Stop. */
function JobProgress({ status, onStop, stopping, canStop }: {
  status: CalibrationDuplicateStatus;
  onStop: () => void;
  stopping: boolean;
  canStop: boolean;
}) {
  const percent = duplicateJobPercent(status);
  const label = duplicateJobLabel(status);
  return (
    <div className="calibration-dup-progress" data-testid="duplicates-progress">
      <div className="calibration-dup-progress-copy">
        <span className="calibration-spinner" aria-hidden="true" />
        <span className="calibration-dup-progress-label">
          <strong>{duplicateJobTitle(status)}</strong> · {label}
          {status.waiting_for && <span className="calibration-muted"> · waiting for {status.waiting_for}</span>}
        </span>
        {canStop && (
          <button type="button" className="tool-button" disabled={stopping} onClick={onStop}
            title={status.action === 'repair' ? 'Stop between exposures; nothing is written' : 'Stop the check'}>
            {stopping ? 'Stopping…' : 'Stop'}
          </button>
        )}
      </div>
      <div className="calibration-dup-progress-track" role="progressbar" aria-label={duplicateJobTitle(status)}
        aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent == null ? undefined : Math.round(percent)}
        aria-valuetext={label}>
        <span style={{ width: `${percent ?? 0}%` }} />
      </div>
    </div>
  );
}

/** What the dialog follows: the check it started for a preview, or the
 *  repair Apply started. */
type DialogRun = { phase: 'preview' | 'repair'; run: number };

/**
 * Exposures the library records more than once — a copy a filer left
 * behind, a second copy in another scanned folder — with the row a repair
 * keeps, and the repair itself: a preview, then Apply. The check and the
 * repair are jobs on the server; this view polls their status.
 */
export default function CalibrationDuplicates({ dbId, canRepair, repairNote, canStop, filters }: CalibrationDuplicatesProps) {
  const queryClient = useQueryClient();
  const clockOf = useRigClocks(dbId);
  const status = useQuery({
    queryKey: duplicatesKey(dbId),
    queryFn: () => apiClient.getCalibrationDuplicates(dbId),
    refetchInterval: (query) => duplicatesPollMs(query.state.data),
  });
  const data = status.data;
  const running = data?.state === 'running';
  const report = data?.report ?? null;
  const [dialog, setDialog] = useState<DialogRun | null>(null);
  // The repair this view started, so its result shows here once.
  const [repairRun, setRepairRun] = useState<number | null>(null);
  // A status read sent before this run started can answer after it, and
  // would put the last run back: drop any read in flight first.
  const store = async (fresh: CalibrationDuplicateStatus) => {
    await queryClient.cancelQueries({ queryKey: duplicatesKey(dbId) });
    queryClient.setQueryData(duplicatesKey(dbId), fresh);
  };

  // `true` for the check the view starts on its own, which runs in the
  // background; a click runs one at once.
  const checking = useMutation({
    mutationFn: (automatic: boolean = false) => apiClient.checkCalibrationDuplicates(dbId, automatic),
    onSuccess: store,
  });
  const repairing = useMutation({
    mutationFn: (token: string) => apiClient.repairCalibrationDuplicates(dbId, token),
    onSuccess: async (fresh) => {
      await store(fresh);
      setRepairRun(fresh.run);
      setDialog({ phase: 'repair', run: fresh.run });
    },
  });
  const stopping = useMutation({
    mutationFn: () => apiClient.stopCalibrationDuplicates(dbId),
    onSuccess: store,
  });

  // With no plan known, or one the catalog has moved on from (a sync, an
  // import, a repair from the command line), the view checks on its own, in
  // the background; so too after a repair applied. A check that stopped or
  // failed, or that the server refused, waits for a person to ask again.
  const autoRun = useRef<number | null>(null);
  useEffect(() => {
    if (!data || data.state === 'running' || checking.isPending) return;
    if (data.report && !data.stale) return;
    if (data.action === 'check' && (data.state === 'stopped' || data.state === 'failed')) return;
    if (autoRun.current === data.run) return;
    autoRun.current = data.run;
    checking.mutate(true);
  }, [data, checking]);

  // A repair that applied leaves the library changed everywhere.
  const repaired = data?.repaired && data.repaired.run === repairRun ? data.repaired : null;
  const repairedRun = repaired?.run;
  useEffect(() => {
    if (repairedRun == null) return;
    setDialog((current) => (current?.phase === 'repair' && current.run === repairedRun ? null : current));
    void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibrations'] });
    void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibration-health'] });
  }, [dbId, queryClient, repairedRun]);

  const rig = filters.rig && filters.rig !== ALL_RIGS ? filters.rig : undefined;
  const groups = useMemo(
    () => (report?.groups ?? []).filter(
      (group) => (!rig || group.rig_uuid === rig) && (!filters.kind || group.kind === filters.kind)
    ),
    [filters.kind, report?.groups, rig]
  );
  const total = report?.groups.length ?? 0;
  const startPreview = () => {
    checking.mutate(false, {
      onSuccess: (fresh) => setDialog({ phase: 'preview', run: fresh.run }),
    });
  };
  const closeDialog = () => {
    setDialog(null);
    repairing.reset();
  };

  // The run the dialog follows, once the status has caught up with it.
  const dialogStatus = dialog && data && data.run === dialog.run ? data : null;
  const dialogRunning = !dialogStatus || dialogStatus.state === 'running';
  const checkStopped = data?.action === 'check' && !running && !report && (data.state === 'stopped' || data.state === 'failed');
  const startError = checking.error ?? repairing.error ?? stopping.error;
  // The check could not start (the server refused it): say so, rather than
  // "Checking the library…" for ever.
  const checkRefused = !!checking.error && !running && !report;

  return (
    <section className="calibration-duplicates" aria-label="Duplicates">
      <div className="calibration-duplicates-head">
        {report && total > 0 && (
          <p className="calibration-muted calibration-count">
            {groups.length === total
              ? plural(total, 'exposure') + ' recorded more than once'
              : `${groups.length} of ${plural(total, 'exposure')} recorded more than once`}
          </p>
        )}
        {!report && !checkStopped && !running && !checkRefused && (
          <p className="calibration-muted calibration-count">Checking the library…</p>
        )}
        {!running && report && (
          <span className="calibration-duplicates-actions">
            {data?.checked_at != null && (
              <span className="calibration-muted calibration-checked-at" title={serverTime(data.checked_at)}>
                Checked {checkedAgo(data.checked_at)}
              </span>
            )}
            <button type="button" className="tool-button" disabled={checking.isPending}
              onClick={() => checking.mutate(false)}>
              Check again
            </button>
            {canRepair && total > 0 && (
              <button type="button" className="tool-button" disabled={checking.isPending} onClick={startPreview}>
                Repair duplicates…
              </button>
            )}
          </span>
        )}
      </div>
      {running && data && !dialog && (
        <JobProgress status={data} canStop={canStop} stopping={stopping.isPending} onStop={() => stopping.mutate()} />
      )}
      {!canRepair && total > 0 && repairNote && <p className="calibration-notice">{repairNote}</p>}
      {repaired && (
        <p className="calibration-notice" role="status">
          Repaired {plural(repaired.outcome.groups, 'exposure')}: {plural(repaired.outcome.rows_removed, 'row')} merged
          {repaired.outcome.repointed > 0 ? `, ${repaired.outcome.repointed} moved to a live copy` : ''}
          {repaired.outcome.masters_removed > 0 ? `, ${plural(repaired.outcome.masters_removed, 'master')} to build again` : ''}.
          No file was moved or deleted.
        </p>
      )}
      {checkStopped && (
        <p className={data?.state === 'failed' ? 'calibration-error' : 'calibration-notice'} role={data?.state === 'failed' ? 'alert' : 'status'}>
          {data?.state === 'failed' ? `Could not check for duplicates: ${data.error ?? 'unknown error'}` : 'The check for duplicates was stopped.'}{' '}
          <button type="button" className="link-button" disabled={checking.isPending} onClick={() => checking.mutate(false)}>
            Check again
          </button>
        </p>
      )}
      {status.error && <p className="calibration-error">Could not check for duplicates: {status.error.message}</p>}
      {startError && !dialog && (
        <p className="calibration-error" role="alert">
          {checking.error ? `Could not check for duplicates: ${checking.error.message}` : startError.message}
        </p>
      )}
      {report && total === 0 && (
        <p className="calibration-empty">No exposure is recorded more than once.</p>
      )}
      {report && total > 0 && groups.length === 0 && (
        <p className="calibration-empty">No duplicates match the rig and type chosen.</p>
      )}
      {groups.length > 0 && (
        <ul className="calibration-twin-groups">
          {groups.map((group) => (
            <li key={group.keep} className="calibration-twin-group" aria-label={group.file_name}>
              <div className="calibration-twin-head">
                <span className={`calibration-kind calibration-kind-${group.kind}`}>{KIND_LABELS[group.kind]}</span>
                <strong>{group.file_name}</strong>
              </div>
              <p className="calibration-muted">
                {[formatTime(group.captured_at, clockOf(group.rig_uuid)), group.rig_name, groupNote(group)].filter(Boolean).join(' · ')}
              </p>
              <ul className="calibration-twin-members">
                {group.members.map((member) => (
                  <li key={member.frame_uuid} className="calibration-twin-member">
                    <span className="calibration-twin-tags">
                      <span className={`calibration-badge${member.keep ? ' calibration-badge-ok' : ''}`}>
                        {member.keep ? 'Keep' : 'Merge'}
                      </span>
                      {!member.exists && <span className="calibration-badge calibration-badge-poor">Missing</span>}
                    </span>
                    <code title={member.frame_uuid}>{member.source_path}</code>
                  </li>
                ))}
              </ul>
              {group.repointed && (
                <p className="calibration-muted calibration-twin-move">
                  The kept row moves to <code>{group.path}</code>
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
      {dialog && (
        <Dialog
          open
          title="Repair duplicates"
          className="calibration-repair-dialog"
          onClose={closeDialog}
          footer={(
            <>
              <button type="button" className="header-button" onClick={closeDialog}>
                {dialogRunning ? 'Close' : 'Cancel'}
              </button>
              {!dialogRunning && dialogStatus?.state === 'done' && dialogStatus.report && dialogStatus.report.groups.length > 0 && (
                <button type="button" className="action-button" disabled={repairing.isPending}
                  onClick={() => repairing.mutate(dialogStatus.report!.token)}>
                  {repairing.isPending ? 'Starting…' : 'Apply'}
                </button>
              )}
            </>
          )}
        >
          <section aria-label="Repair preview" className="calibration-repair-preview">
            {dialogRunning ? (
              <>
                {dialogStatus ? (
                  <JobProgress status={dialogStatus} canStop={canStop} stopping={stopping.isPending}
                    onStop={() => stopping.mutate()} />
                ) : (
                  <p className="calibration-muted">Starting…</p>
                )}
                <p className="calibration-muted">
                  {dialog.phase === 'repair'
                    ? 'The plan is checked against the files once more, then the exposures merge in one transaction. Stop rolls the whole repair back.'
                    : 'Looking at each file the duplicate rows record, and comparing copies that could hold one exposure. Close this and the check goes on; the job line shows it.'}
                </p>
              </>
            ) : dialogStatus?.state === 'stopped' ? (
              <p role="status">
                {dialog.phase === 'repair' ? 'The repair was stopped. Nothing was changed.' : 'The preview was stopped.'}{' '}
                <button type="button" className="tool-button" onClick={startPreview}>Preview again</button>
              </p>
            ) : dialogStatus?.state === 'failed' ? (
              <p className="calibration-error" role="alert">
                {dialogStatus.error ?? 'The run failed.'}{' '}
                <button type="button" className="tool-button" onClick={startPreview}>Preview again</button>
              </p>
            ) : !dialogStatus?.report || dialogStatus.report.groups.length === 0 ? (
              <p>No exposure is recorded more than once any more.</p>
            ) : (
              <>
                {dialogStatus.changed && (
                  <div className="calibration-notice calibration-plan-changed" role="alert">
                    <p>
                      <strong>The plan changed since the preview, so nothing was applied.</strong> This is the plan
                      for the library as it is now; Apply it, or close.
                    </p>
                    <ul>
                      {changeLines(dialogStatus.changed).map((line) => <li key={line}>{line}</li>)}
                    </ul>
                  </div>
                )}
                <ul>
                  {planLines(dialogStatus.report).map((line) => <li key={line}>{line}</li>)}
                </ul>
                <p className="calibration-muted">
                  The oldest row of each exposure stays, with its grade and marks; the others are merged into it.
                  No file is moved or deleted. A second copy on disk is remembered, so later scans skip it.
                </p>
                <p className="calibration-muted">
                  The repair covers every rig and type in this library, not only the groups shown. A catalog this
                  one pulls from keeps its own duplicates until it is repaired there; a pull leaves these rows
                  merged here.
                </p>
                {dialogStatus.report.masters.length > 0 && (
                  <details>
                    <summary>Masters built again</summary>
                    <ul className="calibration-repair-masters">
                      {dialogStatus.report.masters.map((master) => (
                        <li key={master.master_uuid}>
                          {KIND_LABELS[master.kind] ?? master.kind} {shortUuid(master.master_uuid)} · {plural(master.source_count, 'frame')}
                          {' · built '}{serverTime(master.created_at)}
                          {master.doubled ? ' · averaged an exposure twice' : master.direct ? '' : ' · built on one of these'}
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </>
            )}
            {startError && (
              <p className="calibration-error" role="alert">{startError.message}</p>
            )}
          </section>
        </Dialog>
      )}
    </section>
  );
}
