import type { CalibrationFrameDetail, CalibrationValidDirection, FlatClockRun } from '../../api/types';
import {
  KIND_LABELS,
  VALIDITY_LABELS,
  VALIDITY_TITLES,
  fileName,
  formatLimit,
  formatMetric,
  formatSettings,
  formatTime,
  gradeLabel,
  metricLabel,
  plural,
  suspectOverruled,
} from './calibrationLabels';
import type { useCalibrationActions } from './useCalibrationActions';
import { isLarge, spotsSummary } from './dustLabels';

interface CalibrationFramePanelProps {
  detail: CalibrationFrameDetail | undefined;
  /** The detail is the last frame's, shown while the next one loads. */
  stale?: boolean;
  loading: boolean;
  error: string | null;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  /** Forget this frame, once confirmed. The browser sends the request
   *  and chooses the frame to show after it. */
  onForget: (frameUuid: string) => void;
  /** Its rig's clock, which its time reads on. */
  clock?: FlatClockRun[];
}

const VALIDITY: { value: CalibrationValidDirection | 'both'; label: string; title: string }[] = [
  { value: 'backward', label: '◂ Before only', title: VALIDITY_TITLES.backward },
  { value: 'both', label: 'Both', title: 'Serves lights on either side of it.' },
  { value: 'forward', label: 'After only ▸', title: VALIDITY_TITLES.forward },
];

/** One frame: what it is, its grade and the numbers behind it, and what a
 *  person can do with it. */
export default function CalibrationFramePanel({
  detail,
  stale = false,
  loading,
  error,
  canManage,
  actions,
  onForget,
  clock,
}: CalibrationFramePanelProps) {
  if (!detail) {
    return (
      <aside className="calibration-panel" aria-label="Frame">
        <p className="calibration-muted">{error ? `Could not read the frame: ${error}` : loading ? 'Loading…' : 'No frame chosen.'}</p>
      </aside>
    );
  }
  const name = fileName(detail.source_path);
  const grade = gradeLabel(detail);
  // A suspect frame used anyway reads as OK: its grade gives way to the
  // person's choice.
  const overruled = suspectOverruled(detail);
  const uuids = [detail.frame_uuid];
  const forget = () => {
    if (window.confirm(`Remove the catalog record for ${name} and the masters built from this frame? The source file will remain on disk.`)) {
      onForget(detail.frame_uuid);
    }
  };
  return (
    <aside className={`calibration-panel${stale ? ' is-stale' : ''}`} aria-label="Frame" aria-busy={stale}>
      <header>
        <h2 title={detail.source_path}>{name}</h2>
        <p className="calibration-muted calibration-path" title={detail.source_path}>{detail.source_path}</p>
        <p>{KIND_LABELS[detail.kind]} · {formatTime(detail.captured_at, clock)}</p>
        <p className="calibration-muted">{formatSettings(detail)}</p>
        {detail.camera && <p className="calibration-muted">{detail.camera}</p>}
      </header>

      <div className="calibration-badges">
        {grade && !overruled && <span className={`calibration-badge calibration-badge-${detail.grade}`} title={detail.grade_reason ?? 'Nothing crossed a limit.'}>{grade}</span>}
        {!detail.grade && detail.stray_light && <span className="calibration-badge calibration-badge-poor" title={detail.stray_light}>Stray light</span>}
        {!detail.grade && !detail.stray_light && <span className="calibration-badge">Not graded</span>}
        {detail.quality_override === 'exclude' && <span className="calibration-badge calibration-badge-poor">Kept out by hand</span>}
        {detail.quality_override === 'use' && (
          <span className="calibration-badge calibration-badge-ok"
            title={overruled ? 'Graded suspect; a person found it fine, so it counts as OK' : 'Used in masters and exports whatever its grade'}>
            Used anyway
          </span>
        )}
        {detail.valid_direction && (
          <span className="calibration-badge calibration-badge-suspect" title={VALIDITY_TITLES[detail.valid_direction]}>
            {VALIDITY_LABELS[detail.valid_direction]}
          </span>
        )}
        {detail.is_master && <span className="calibration-badge">Master</span>}
        {!detail.source_exists && <span className="calibration-badge calibration-badge-poor">Missing file</span>}
      </div>
      {detail.kept_out && <p className="calibration-reason">Left out of masters: {detail.kept_out}</p>}
      {!detail.kept_out && detail.grade_reason && (
        <p className={overruled ? 'calibration-muted' : 'calibration-reason'}>
          {overruled ? `Suspect grade overruled: ${detail.grade_reason}` : detail.grade_reason}
        </p>
      )}
      {detail.measure_error && <p className="calibration-reason">Not measured: {detail.measure_error}</p>}

      {detail.metrics.length > 0 && (
        <table className="calibration-numbers">
          <thead>
            <tr>
              <th scope="col">Measure</th>
              <th scope="col">Frame</th>
              <th scope="col">Session</th>
            </tr>
          </thead>
          <tbody>
            {detail.metrics.filter((metric) => metric.key !== 'large_dust').map((metric) => {
              // A flat's spots and its large dust read as one: `15 spots ·
              // 3 large disks`, the frame's and its session's.
              const large = metric.key === 'features'
                ? detail.metrics.find((other) => other.key === 'large_dust')
                : undefined;
              const shapes = (detail.measurement?.features ?? [])
                .map((feature) => feature.shape ?? 'compact')
                .filter(isLarge);
              return (
                <tr key={metric.key}>
                  <th scope="row">{metricLabel(metric.key)}</th>
                  <td>{large ? spotsSummary(metric.value, large.value, shapes) : formatMetric(metric.key, metric.value)}</td>
                  <td>{large ? spotsSummary(metric.session, large.session, shapes) : formatMetric(metric.key, metric.session)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {detail.findings.length > 0 && (
        <ul className="calibration-findings" aria-label="Findings">
          {detail.findings.map((finding, index) => (
            // Overruled, the numbers stay as plain evidence, with no grade
            // word or colour to read as a flag.
            overruled ? (
              <li key={`${finding.code}:${index}`} className="calibration-muted">
                {finding.text} · limit {formatLimit(finding)}
              </li>
            ) : (
              <li key={`${finding.code}:${index}`} className={`calibration-finding-${finding.grade}`}>
                <strong>{finding.grade === 'poor' ? 'Poor' : 'Suspect'}</strong> {finding.text}
                <span className="calibration-muted"> · limit {formatLimit(finding)}</span>
              </li>
            )
          ))}
        </ul>
      )}
      <p className="calibration-muted">
        {detail.session_measured} of {plural(detail.session_frames, 'frame')} in its session measured
        {detail.masters > 0 && ` · in ${plural(detail.masters, 'master')}`}
      </p>

      {canManage && (
        <div className="calibration-actions">
          {/* Keep out is a choice made by hand, like rejecting a light: it
              holds whatever later grades say. So it is offered even when the
              grade already keeps the frame out. */}
          <div className="calibration-action-row">
            {detail.quality_override !== 'exclude' && (
              <button type="button" className="tool-button" disabled={actions.pending || stale}
                title={detail.kept_out
                  ? 'Exclude from masters and exports even if its grade changes; keep the frame in the library'
                  : 'Leave out of masters and exports; the frame stays in the library'}
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'exclude' })}>
                Keep out
              </button>
            )}
            {/* A suspect frame stays in masters, but is flagged: Use anyway
                overrules the flag, as it overrules a grade that keeps one out. */}
            {detail.quality_override !== 'use' && (detail.kept_out || detail.grade === 'suspect') && (
              <button type="button" className="tool-button" disabled={actions.pending || stale}
                title={detail.kept_out
                  ? 'Use in masters and exports whatever its grade'
                  : 'Use anyway: this frame is fine; ignore the suspect grade'}
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'use' })}>
                Use anyway
              </button>
            )}
            {detail.quality_override && (
              <button type="button" className="tool-button" disabled={actions.pending || stale}
                title="Let the grade decide again"
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'clear' })}>
                Clear
              </button>
            )}
            <button type="button" className="tool-button tool-button-danger"
              disabled={actions.pending || stale} onClick={forget}>
              Forget
            </button>
          </div>
          <div className="tool-segmented" role="group" aria-label="Serves lights">
            {VALIDITY.map((option) => (
              <button key={option.value} type="button" title={option.title} disabled={actions.pending || stale}
                aria-pressed={(detail.valid_direction ?? 'both') === option.value}
                onClick={() => actions.mark.mutate({ frameUuids: uuids, direction: option.value })}>
                {option.label}
              </button>
            ))}
          </div>
        </div>
      )}
      {actions.error && <p className="calibration-error" role="alert">{actions.error}</p>}
    </aside>
  );
}
