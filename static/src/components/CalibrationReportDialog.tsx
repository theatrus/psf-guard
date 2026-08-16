import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { CalibrationNightFilter, ProjectCalibrationReport } from '../api/types';
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

function flatCell(filter: CalibrationNightFilter): string {
  if (filter.flat_frames === 0) return 'none';
  const session = filter.flat_session ?? '?';
  return filter.nightly_flats
    ? `${filter.flat_frames} · same night`
    : `${filter.flat_frames} · ${session} (${formatAge(filter.flat_age_days)})`;
}

function darkCell(filter: CalibrationNightFilter): string {
  if (filter.dark_frames === 0) return 'none';
  return `${filter.dark_frames} · ${formatAge(filter.dark_age_days)}`;
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
                        <td className={filter.dark_frames === 0 ? 'missing' : ''}>
                          {darkCell(filter)}
                        </td>
                        <td className={filter.bias_frames === 0 ? 'missing' : ''}>
                          {filter.bias_frames || 'none'}
                        </td>
                      </tr>
                    ))
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
          </>
        )}
      </div>
    </Dialog>
  );
}
