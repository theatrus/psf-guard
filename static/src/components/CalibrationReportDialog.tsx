import { useState, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../api/client';
import { useAccess } from '../auth/access';
import type { CalibrationExternalMaster, CalibrationHealthFinding, CalibrationHealthLevel, CalibrationNightFilter, ProjectCalibrationGaps, ProjectCalibrationReport } from '../api/types';
import Dialog from './Dialog';
import { calibrationGridHref, calibrationPageHref, nightDates } from './calibration/calibrationLinks';
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

/** Open the Images grid on the lights a calibration level or finding code
 *  covers, in the given filters or all of them, and in one night
 *  (`YYYY-MM-DD`) or all of them. */
type ShowLights = (calib: string, filterNames: string[], night?: string) => void;

/** The text of a finding as a link to its lights in the Images grid. */
function LightsLink({ onShow, calib, filterNames, night, children }: {
  onShow: ShowLights;
  calib: string;
  filterNames: string[];
  night?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className="calibration-report-link"
      title="Show these lights in Images"
      onClick={() => onShow(calib, filterNames, night)}
    >
      {children}
    </button>
  );
}

/**
 * The lights a stack leaves out because the library cannot calibrate them,
 * found on request (every light is read), and rejected after that check so
 * Target Scheduler shoots them again.
 */
function UncalibratedLights({ dbId, projectId, onShow }: { dbId: string; projectId: number; onShow: ShowLights }) {
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
    ? Object.entries(gaps.lights.reduce<Record<string, { night: string; filter: string; count: number }>>((counts, light) => {
      const key = `${light.night} · ${light.filter || '—'} · ${light.reason}`;
      counts[key] = { night: light.night, filter: light.filter, count: (counts[key]?.count ?? 0) + 1 };
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
        No matching flat, no bias or dark, or flats past the age limit. Rejecting them lets Target
        Scheduler shoot them again; Settings › Calibration can also leave them out of stacks.
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
              {counted.map(([key, { night, filter, count }]) => (
                <li key={key}>
                  <LightsLink onShow={onShow} calib="cannot_calibrate" filterNames={filter ? [filter] : []} night={night}>
                    {key}: {count}
                  </LightsLink>
                </li>
              ))}
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

const LEVEL_WORDS: Record<CalibrationHealthLevel, string> = { poor: 'Poor', check: 'Check' };

const nightsText = (finding: CalibrationHealthFinding) =>
  finding.nights.map((night) => `${night.night} ${night.filter || '—'}: ${night.lights}`).join(' · ');

/** The filters a finding's lights were taken through; none when one of
 *  them has no filter, since the grid cannot pick that out. */
const findingFilters = (finding: CalibrationHealthFinding) => {
  const filters = finding.nights.map((night) => night.filter);
  return filters.includes('') ? [] : [...new Set(filters)].sort();
};

/**
 * What the background check found for this project: each finding with its
 * lights by night and filter. Read from the kept result; never worked out
 * here, so a project the check has not reached says so.
 */
function QualitySection({ dbId, projectId, onShow }: { dbId: string; projectId: number; onShow: ShowLights }) {
  const query = useQuery({
    queryKey: ['db', dbId, 'project', projectId, 'calibration-health'],
    queryFn: () => apiClient.getProjectCalibrationHealth(dbId, projectId),
    staleTime: 30_000,
  });
  const health = query.data?.health;
  return (
    <section className="calibration-report-quality" aria-label="Quality">
      <strong>Quality</strong>
      {query.isLoading && <p className="calibration-report-muted">Reading…</p>}
      {query.error && <p className="calibration-report-error">{message(query.error)}</p>}
      {query.data && !health && (
        <p className="calibration-report-muted">
          {query.data.running ? 'Being checked now.' : 'Not checked yet.'}
        </p>
      )}
      {health && health.findings.length === 0 && (
        <p>Nothing found in {lights(health.checked)}.</p>
      )}
      {health && health.findings.length > 0 && (
        <>
          <p>
            {health.poor_lights > 0
              ? `${health.poor_lights.toLocaleString()} of ${lights(health.checked)} can't be calibrated properly${health.check_lights > 0 ? `; ${health.check_lights.toLocaleString()} more to check` : ''}.`
              : `${health.check_lights.toLocaleString()} of ${lights(health.checked)} to check.`}
          </p>
          <ul className="calibration-report-findings">
            {health.findings.map((finding) => (
              <li key={`${finding.level}:${finding.code}:${finding.reason}`}>
                <span className={`calibration-report-level is-${finding.level}`}>{LEVEL_WORDS[finding.level]}</span>
                {' '}
                <LightsLink onShow={onShow} calib={finding.code} filterNames={findingFilters(finding)}>
                  {finding.reason}: {lights(finding.lights)}
                </LightsLink>
                <small className="calibration-report-muted">{nightsText(finding)}</small>
              </li>
            ))}
          </ul>
        </>
      )}
      {health && health.missing_files > 0 && (
        <p className="calibration-report-muted">{lights(health.missing_files)} not found or not readable.</p>
      )}
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
  const [includeRejected, setIncludeRejected] = useState(false);
  const report = useQuery<ProjectCalibrationReport>({
    queryKey: ['db', dbId, 'project', projectId, 'calibration-report', { includeRejected }],
    queryFn: () => apiClient.getProjectCalibrationReport(dbId, projectId, includeRejected),
    enabled: open,
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  });
  const navigate = useNavigate();
  const location = useLocation();
  const client = useQueryClient();
  const browse = () => {
    onClose();
    navigate(calibrationPageHref(location.search, dbId, projectId));
  };
  const showLights: ShowLights = (calib, filterNames, night) => {
    const show = (boundary?: number) => {
      onClose();
      const dates = night !== undefined && boundary !== undefined ? nightDates(night, boundary) : undefined;
      navigate(calibrationGridHref(location.search, dbId, projectId, calib, filterNames, dates));
    };
    if (night === undefined) {
      show();
      return;
    }
    // A night's link carries that night's span, which depends on where this
    // catalog's nights split. Should that not come, all nights show.
    client.fetchQuery({
      queryKey: ['db', dbId, 'calibrations', 'night-boundary'],
      queryFn: () => apiClient.getCalibrationSessions(dbId, { per_page: 1 }).then((page) => page.night_boundary_seconds),
      staleTime: 5 * 60 * 1000,
    }).then(show, () => show());
  };

  return (
    <Dialog open={open} onClose={onClose} title={`Calibration coverage — ${projectName}`}>
      <div className="calibration-report">
        <button type="button" className="calibration-report-browse" onClick={browse}>
          Browse calibration
        </button>
        <QualitySection dbId={dbId} projectId={projectId} onShow={showLights} />
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

            {report.data.rejected_lights > 0 && (
              <label className="calibration-report-rejected">
                <input
                  type="checkbox"
                  checked={includeRejected}
                  onChange={(event) => setIncludeRejected(event.target.checked)}
                />
                Include {report.data.rejected_lights.toLocaleString()} rejected light{report.data.rejected_lights === 1 ? '' : 's'}
              </label>
            )}

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
                        <td colSpan={5} className="missing">
                          <LightsLink onShow={showLights} calib="cannot_calibrate" filterNames={filter.filter ? [filter.filter] : []} night={night.night}>
                            {filter.filter}: can't be calibrated · {filter.cannot_calibrate}
                          </LightsLink>
                        </td>
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
            <UncalibratedLights dbId={dbId} projectId={projectId} onShow={showLights} />
          </>
        )}
      </div>
    </Dialog>
  );
}
