import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import { useAccess } from '../auth/access';
import type { CalibrationExternalMaster, CalibrationNightFilter, ProjectCalibrationGaps, ProjectCalibrationReport } from '../api/types';
import Dialog from './Dialog';
import './CalibrationReportDialog.css';

interface Props {
  open: boolean;
  dbId: string;
  projectId: number;
  projectName: string;
  onClose: () => void;
}

function formatDay(timestamp: number | null | undefined): string {
  if (timestamp == null) return '—';
  return new Date(timestamp * 1000).toISOString().slice(0, 10);
}

function formatAge(days: number | null | undefined): string {
  if (days == null) return '—';
  if (days < 1) return 'same night';
  return `${Math.round(days)} d away`;
}

const angle = (value: number) => `${Math.round(value * 10) / 10}°`;

function flatCell(filter: CalibrationNightFilter): string {
  if (filter.flat_frames === 0) {
    const miss = filter.flat_near_miss;
    return miss
      ? `none · nearest ${miss.filter ?? ''} ${angle(miss.flat_rotation_deg)}, ${angle(miss.off_by_deg)} off (limit ${angle(miss.tolerance_deg)})${miss.session ? ` · ${miss.session}` : ''}`
      : 'none';
  }
  const session = filter.flat_session ?? '?';
  return filter.nightly_flats
    ? `${filter.flat_frames} · same night`
    : `${filter.flat_frames} · ${session} (${formatAge(filter.flat_age_days)})`;
}

function darkCell(filter: CalibrationNightFilter): string {
  if (filter.dark_frames === 0) return 'none';
  const master = filter.dark_master_frames ?? filter.dark_frames;
  const nights = filter.dark_master_nights ?? 1;
  return `${master} · ${nights > 1 ? `${nights} nights, nearest ${formatAge(filter.dark_age_days)}` : formatAge(filter.dark_age_days)}`;
}

function masterNote(master: CalibrationExternalMaster): string {
  const state = master.used ? 'used' : master.matches ? 'matches, not used' : `not used: ${master.reason ?? 'no match'}`;
  return `${master.kind} master ${master.file} · ${state}`;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'The request failed.';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const lights = (count: number) => `${count.toLocaleString()} light${count === 1 ? '' : 's'}`;

/**
 * The lights a stack leaves out because the library cannot calibrate them,
 * found on request (every light is read), and rejected after that check so
 * Target Scheduler shoots them again.
 */
function UncalibratedLights({ dbId, projectId }: { dbId: string; projectId: number }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [notice, setNotice] = useState('');
  const check = useMutation({
    retry: false,
    mutationFn: () => apiClient.getProjectCalibrationGaps(dbId, projectId),
    onSuccess: () => setNotice(''),
  });
  const reject = useMutation({
    retry: false,
    mutationFn: (gaps: ProjectCalibrationGaps) => apiClient.rejectUncalibratedLights(dbId, projectId, gaps.digest),
    onSuccess: (report) => {
      check.reset();
      setNotice(`Rejected ${lights(report.updated)}; Target Scheduler will shoot them again.`);
      void client.invalidateQueries({ queryKey: ['db', dbId] });
    },
    onError: (error) => {
      if (httpStatus(error) === 409) {
        check.reset();
        setNotice('The lights changed since the check; check again.');
      }
    },
  });
  const gaps = check.data;
  const counted = gaps
    ? Object.entries(gaps.lights.reduce<Record<string, number>>((counts, light) => {
      const key = `${light.night} · ${light.filter || '—'} · ${light.reason}`;
      counts[key] = (counts[key] ?? 0) + 1;
      return counts;
    }, {}))
    : [];
  const busy = check.isPending || reject.isPending;
  const error = check.error ?? (reject.error && httpStatus(reject.error) !== 409 ? reject.error : null);
  return (
    <section className="calibration-report-gaps" aria-label="Lights that can't be calibrated">
      <div className="calibration-report-gaps-row">
        <strong>Lights that can't be calibrated</strong>
        <button type="button" disabled={busy} onClick={() => check.mutate()}>
          {check.isPending ? 'Checking…' : 'Check lights'}
        </button>
      </div>
      <small className="calibration-report-muted">
        No matching flat, no bias or dark, or flats past the age limit. Stacks leave them out;
        rejecting them lets Target Scheduler shoot them again.
      </small>
      {gaps && (
        <>
          <p>
            {gaps.lights.length > 0
              ? `${lights(gaps.lights.length)} of ${gaps.checked.toLocaleString()} can't be calibrated.`
              : `All ${lights(gaps.checked)} can be calibrated.`}
            {gaps.missing_files > 0 && ` ${lights(gaps.missing_files)} not found on disk.`}
          </p>
          {counted.length > 0 && (
            <ul className="calibration-report-gap-list">
              {counted.map(([key, count]) => <li key={key}>{key}: {count}</li>)}
            </ul>
          )}
          {gaps.lights.length > 0 && canWrite && (
            <button type="button" disabled={busy} onClick={() => reject.mutate(gaps)}>
              {reject.isPending ? 'Rejecting…' : `Reject ${lights(gaps.lights.length)}`}
            </button>
          )}
        </>
      )}
      {notice && <p role="status">{notice}</p>}
      {error && <p className="calibration-report-error">{message(error)}</p>}
    </section>
  );
}

/**
 * How the calibration library covers one project: what matches its lights,
 * how old it is, and whether each night has its own flats. Read-only; the
 * matching is exactly what a stack build would resolve.
 */
export default function CalibrationReportDialog({
  open,
  dbId,
  projectId,
  projectName,
  onClose,
}: Props) {
  const report = useQuery<ProjectCalibrationReport>({
    queryKey: ['db', dbId, 'project', projectId, 'calibration-report'],
    queryFn: () => apiClient.getProjectCalibrationReport(dbId, projectId),
    enabled: open,
    staleTime: 60_000,
  });

  return (
    <Dialog open={open} onClose={onClose} title={`Calibration coverage — ${projectName}`}>
      <div className="calibration-report">
        {report.isLoading && <p className="calibration-report-muted">Matching the library…</p>}
        {report.error && (
          <p className="calibration-report-error">
            {report.error instanceof Error ? report.error.message : String(report.error)}
          </p>
        )}
        {report.data && (
          <>
            {report.data.warnings.length > 0 && (
              <ul className="calibration-report-warnings">
                {report.data.warnings.map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            )}

            <div className="calibration-report-kinds">
              {report.data.kinds.map((kind) => (
                <div key={kind.kind} className="calibration-report-kind">
                  <strong>{kind.kind.replace('_', '-')}</strong>
                  {kind.matching_frames === 0 ? (
                    <span className="calibration-report-muted">no matches</span>
                  ) : (
                    <span>
                      {kind.matching_frames} frame{kind.matching_frames === 1 ? '' : 's'} ·{' '}
                      {kind.sessions.length} session{kind.sessions.length === 1 ? '' : 's'}
                      {kind.newest_at != null && ` · newest ${formatDay(kind.newest_at)}`}
                    </span>
                  )}
                </div>
              ))}
            </div>

            <div className="calibration-report-table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>Night</th>
                    <th>Filter</th>
                    <th>Lights</th>
                    <th>Flats</th>
                    <th>Darks</th>
                    <th>Bias</th>
                  </tr>
                </thead>
                <tbody>
                  {report.data.nights.flatMap((night) =>
                    night.filters.map((filter, index) => (
                      <tr key={`${night.night}:${filter.filter}`}>
                        <td>{index === 0 ? night.night : ''}</td>
                        <td>{filter.filter || '—'}</td>
                        <td>{filter.lights}</td>
                        <td className={filter.flat_frames === 0 ? 'missing' : filter.nightly_flats ? 'nightly' : ''}>
                          {flatCell(filter)}
                        </td>
                        <td className={filter.dark_frames === 0 ? 'missing' : ''} title={`${filter.dark_frames} matching darks within reach`}>
                          {darkCell(filter)}
                        </td>
                        <td className={filter.bias_frames === 0 ? 'missing' : ''}>
                          {filter.bias_frames || 'none'}
                        </td>
                      </tr>
                    )).concat(night.filters.filter((filter) => filter.cannot_calibrate).map((filter) => (
                      <tr key={`${night.night}:${filter.filter}:gap`} className="calibration-report-masters">
                        <td />
                        <td colSpan={5} className="missing">{filter.filter}: left out of stacks · {filter.cannot_calibrate}</td>
                      </tr>
                    ))).concat(night.filters.filter((filter) => (filter.external_masters ?? []).length > 0).map((filter) => (
                      <tr key={`${night.night}:${filter.filter}:masters`} className="calibration-report-masters">
                        <td />
                        <td colSpan={5}>
                          <ul>{(filter.external_masters ?? []).map((master) => <li key={`${master.kind}:${master.file}`} className={master.used ? 'nightly' : master.matches ? '' : 'missing'}>{filter.filter}: {masterNote(master)}</li>)}</ul>
                        </td>
                      </tr>
                    )))
                  )}
                </tbody>
              </table>
            </div>

            {report.data.lights_missing_files > 0 && (
              <p className="calibration-report-muted">
                {report.data.lights_missing_files} light(s) were not found on disk and are not
                reported.
              </p>
            )}
            <UncalibratedLights dbId={dbId} projectId={projectId} />
          </>
        )}
      </div>
    </Dialog>
  );
}
