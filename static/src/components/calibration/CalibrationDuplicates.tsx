import { useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CalibrationDuplicateReport,
  CalibrationRepairOutcome,
  CalibrationTwinGroup,
} from '../../api/types';
import Dialog from '../Dialog';
import { KIND_LABELS, formatTime, plural } from './calibrationLabels';
import { ALL_RIGS, type CalibrationFilterState } from './useCalibrationUrlState';

interface CalibrationDuplicatesProps {
  dbId: string;
  /** Whether this account may change the library and the server allows
   *  database management. */
  canRepair: boolean;
  /** Why it may not, when it may not. */
  repairNote: string | null;
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

/**
 * Exposures the library records more than once — a copy a filer left
 * behind, a second copy in another scanned folder — with the row a repair
 * keeps, and the repair itself: a preview, then Apply.
 */
export default function CalibrationDuplicates({ dbId, canRepair, repairNote, filters }: CalibrationDuplicatesProps) {
  const queryClient = useQueryClient();
  const report = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'duplicates'],
    queryFn: () => apiClient.getCalibrationDuplicates(dbId),
  });
  const [preview, setPreview] = useState<CalibrationDuplicateReport | null>(null);
  const [outcome, setOutcome] = useState<CalibrationRepairOutcome | null>(null);
  const previewing = useMutation({
    mutationFn: () => apiClient.getCalibrationDuplicates(dbId),
    onSuccess: (fresh) => {
      setPreview(fresh);
      setOutcome(null);
      applying.reset();
      queryClient.setQueryData(['db', dbId, 'calibrations', 'duplicates'], fresh);
    },
  });
  const applying = useMutation({
    mutationFn: (token: string) => apiClient.repairCalibrationDuplicates(dbId, token),
    onSuccess: (result) => {
      setOutcome(result);
      setPreview(null);
      void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibrations'] });
      void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibration-health'] });
    },
  });

  const rig = filters.rig && filters.rig !== ALL_RIGS ? filters.rig : undefined;
  const groups = useMemo(
    () => (report.data?.groups ?? []).filter(
      (group) => (!rig || group.rig_uuid === rig) && (!filters.kind || group.kind === filters.kind)
    ),
    [filters.kind, report.data?.groups, rig]
  );
  const total = report.data?.groups.length ?? 0;
  const busy = previewing.isPending || applying.isPending;
  const closePreview = () => {
    if (applying.isPending) return;
    setPreview(null);
    applying.reset();
  };

  return (
    <section className="calibration-duplicates" aria-label="Duplicates">
      <div className="calibration-duplicates-head">
        {(!report.data || total > 0) && (
          <p className="calibration-muted calibration-count">
            {report.data
              ? groups.length === total
                ? plural(total, 'exposure') + ' recorded more than once'
                : `${groups.length} of ${plural(total, 'exposure')} recorded more than once`
              : 'Checking the library…'}
          </p>
        )}
        {canRepair && total > 0 && (
          <button type="button" className="tool-button" disabled={busy} onClick={() => previewing.mutate()}>
            {previewing.isPending ? 'Preparing preview…' : 'Repair duplicates…'}
          </button>
        )}
      </div>
      {!canRepair && total > 0 && repairNote && <p className="calibration-notice">{repairNote}</p>}
      {outcome && (
        <p className="calibration-notice" role="status">
          Repaired {plural(outcome.groups, 'exposure')}: {plural(outcome.rows_removed, 'row')} merged
          {outcome.repointed > 0 ? `, ${outcome.repointed} moved to a live copy` : ''}
          {outcome.masters_removed > 0 ? `, ${plural(outcome.masters_removed, 'master')} to build again` : ''}.
          No file was moved or deleted.
        </p>
      )}
      {report.error && <p className="calibration-error">Could not check for duplicates: {report.error.message}</p>}
      {previewing.error && <p className="calibration-error" role="alert">{previewing.error.message}</p>}
      {report.data && total === 0 && (
        <p className="calibration-empty">No exposure is recorded more than once.</p>
      )}
      {report.data && total > 0 && groups.length === 0 && (
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
                {[formatTime(group.captured_at), group.rig_name, groupNote(group)].filter(Boolean).join(' · ')}
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
      {preview && (
        <Dialog
          open
          title="Repair duplicates"
          className="calibration-repair-dialog"
          onClose={closePreview}
          footer={(
            <>
              <button type="button" className="header-button" disabled={applying.isPending} onClick={closePreview}>Cancel</button>
              {preview.groups.length > 0 && (
                <button type="button" className="action-button" disabled={busy}
                  onClick={() => applying.mutate(preview.token)}>
                  {applying.isPending ? 'Applying…' : 'Apply'}
                </button>
              )}
            </>
          )}
        >
          <section aria-label="Repair preview" className="calibration-repair-preview">
            {preview.groups.length === 0 ? (
              <p>No exposure is recorded more than once any more.</p>
            ) : (
              <>
                <ul>
                  {planLines(preview).map((line) => <li key={line}>{line}</li>)}
                </ul>
                <p className="calibration-muted">
                  The oldest row of each exposure stays, with its grade and marks; the others are merged into it.
                  No file is moved or deleted. A second copy on disk is remembered, so later scans skip it.
                </p>
                {preview.masters.length > 0 && (
                  <details>
                    <summary>Masters built again</summary>
                    <ul className="calibration-repair-masters">
                      {preview.masters.map((master) => (
                        <li key={master.master_uuid}>
                          {KIND_LABELS[master.kind] ?? master.kind} {shortUuid(master.master_uuid)} · {plural(master.source_count, 'frame')}
                          {' · built '}{formatTime(master.created_at)}
                          {master.doubled ? ' · averaged an exposure twice' : master.direct ? '' : ' · built on one of these'}
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </>
            )}
            {applying.error && (
              <p className="calibration-error" role="alert">
                {applying.error.message}{' '}
                <button type="button" className="tool-button" disabled={busy} onClick={() => previewing.mutate()}>
                  Preview again
                </button>
              </p>
            )}
          </section>
        </Dialog>
      )}
    </section>
  );
}
