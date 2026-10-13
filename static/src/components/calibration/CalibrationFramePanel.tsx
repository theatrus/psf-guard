import type { CalibrationFrameDetail, CalibrationValidDirection } from '../../api/types';
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
} from './calibrationLabels';
import type { useCalibrationActions } from './useCalibrationActions';

interface CalibrationFramePanelProps {
  detail: CalibrationFrameDetail | undefined;
  /** The detail is the last frame's, shown while the next one loads. */
  stale?: boolean;
  loading: boolean;
  error: string | null;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  onForgotten: () => void;
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
  onForgotten,
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
  const uuids = [detail.frame_uuid];
  const forget = () => {
    if (window.confirm(`Forget ${name}? Its catalog record and the masters built from it go. The file stays.`)) {
      actions.forget.mutate(uuids, { onSuccess: onForgotten });
    }
  };
  return (
    <aside className={`calibration-panel${stale ? ' is-stale' : ''}`} aria-label="Frame" aria-busy={stale}>
      <header>
        <h2 title={detail.source_path}>{name}</h2>
        <p className="calibration-muted calibration-path" title={detail.source_path}>{detail.source_path}</p>
        <p>{KIND_LABELS[detail.kind]} · {formatTime(detail.captured_at)}</p>
        <p className="calibration-muted">{formatSettings(detail)}</p>
        {detail.camera && <p className="calibration-muted">{detail.camera}</p>}
      </header>

      <div className="calibration-badges">
        {grade && <span className={`calibration-badge calibration-badge-${detail.grade}`} title={detail.grade_reason ?? 'Nothing crossed a limit.'}>{grade}</span>}
        {!detail.grade && detail.stray_light && <span className="calibration-badge calibration-badge-poor" title={detail.stray_light}>Stray light</span>}
        {!detail.grade && !detail.stray_light && <span className="calibration-badge">Not graded</span>}
        {detail.quality_override === 'exclude' && <span className="calibration-badge calibration-badge-poor">Kept out by hand</span>}
        {detail.quality_override === 'use' && <span className="calibration-badge calibration-badge-ok">Used anyway</span>}
        {detail.valid_direction && (
          <span className="calibration-badge calibration-badge-suspect" title={VALIDITY_TITLES[detail.valid_direction]}>
            {VALIDITY_LABELS[detail.valid_direction]}
          </span>
        )}
        {detail.is_master && <span className="calibration-badge">Master</span>}
        {!detail.source_exists && <span className="calibration-badge calibration-badge-poor">Missing file</span>}
      </div>
      {detail.kept_out && <p className="calibration-reason">Left out of masters: {detail.kept_out}</p>}
      {!detail.kept_out && detail.grade_reason && <p className="calibration-reason">{detail.grade_reason}</p>}
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
            {detail.metrics.map((metric) => (
              <tr key={metric.key}>
                <th scope="row">{metricLabel(metric.key)}</th>
                <td>{formatMetric(metric.key, metric.value)}</td>
                <td>{formatMetric(metric.key, metric.session)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {detail.findings.length > 0 && (
        <ul className="calibration-findings" aria-label="Findings">
          {detail.findings.map((finding, index) => (
            <li key={`${finding.code}:${index}`} className={`calibration-finding-${finding.grade}`}>
              <strong>{finding.grade === 'poor' ? 'Poor' : 'Suspect'}</strong> {finding.text}
              <span className="calibration-muted"> · limit {formatLimit(finding)}</span>
            </li>
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
              <button type="button" className="calibration-button" disabled={actions.pending || stale}
                title={detail.kept_out
                  ? 'Keep out by hand, whatever later grades say; the frame stays in the library'
                  : 'Leave out of masters and exports; the frame stays in the library'}
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'exclude' })}>
                Keep out
              </button>
            )}
            {detail.quality_override !== 'use' && detail.kept_out && (
              <button type="button" className="calibration-button" disabled={actions.pending || stale}
                title="Use in masters and exports whatever its grade"
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'use' })}>
                Use anyway
              </button>
            )}
            {detail.quality_override && (
              <button type="button" className="calibration-button" disabled={actions.pending || stale}
                title="Let the grade decide again"
                onClick={() => actions.choose.mutate({ frameUuids: uuids, choice: 'clear' })}>
                Clear
              </button>
            )}
            <button type="button" className="calibration-button calibration-button-danger"
              disabled={actions.pending || stale} onClick={forget}>
              Forget
            </button>
          </div>
          <div className="calibration-segmented" role="group" aria-label="Serves lights">
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
