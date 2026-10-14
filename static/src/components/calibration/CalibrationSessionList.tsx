import type { CalibrationSession } from '../../api/types';
import { nightLabel, plural, sessionDetails, sessionTitle } from './calibrationLabels';

interface CalibrationSessionListProps {
  sessions: CalibrationSession[];
  selected: string | null;
  showRig: boolean;
  page: number;
  perPage: number;
  total: number;
  onSelect: (id: string) => void;
  onPage: (page: number) => void;
}

/** Sessions, newest night first, each with its frames counted by grade. */
export default function CalibrationSessionList({
  sessions,
  selected,
  showRig,
  page,
  perPage,
  total,
  onSelect,
  onPage,
}: CalibrationSessionListProps) {
  const first = page * perPage + 1;
  const last = Math.min(total, (page + 1) * perPage);
  return (
    <nav className="calibration-sessions" aria-label="Sessions">
      <ul>
        {sessions.map((session) => (
          <li key={session.id}>
            <button
              type="button"
              aria-current={session.id === selected ? 'true' : undefined}
              data-session-id={session.id}
              onClick={() => onSelect(session.id)}
            >
              <span className="calibration-session-head">
                <strong>{sessionTitle(session)}</strong>
                <span>{nightLabel(session.night)}</span>
              </span>
              <span className="calibration-muted">
                {[showRig ? session.rig_name : null, sessionDetails(session)].filter(Boolean).join(' · ')}
              </span>
              <span className="calibration-session-counts">
                <span>{plural(session.frames, 'frame')}</span>
                {session.poor > 0 && <span className="calibration-badge calibration-badge-poor">{session.poor} poor</span>}
                {session.suspect > 0 && <span className="calibration-badge calibration-badge-suspect">{session.suspect} suspect</span>}
                {session.kept_out > 0 && <span className="calibration-badge calibration-badge-poor">{session.kept_out} out</span>}
                {session.ungraded > 0 && session.ungraded < session.frames && (
                  <span className="calibration-badge">{session.ungraded} not graded</span>
                )}
              </span>
            </button>
          </li>
        ))}
      </ul>
      {total > perPage && (
        <div className="calibration-pager">
          <button type="button" className="tool-button" disabled={page === 0} onClick={() => onPage(page - 1)}>
            Newer
          </button>
          <span>{first}–{last} of {total}</span>
          <button type="button" className="tool-button" disabled={last >= total} onClick={() => onPage(page + 1)}>
            Older
          </button>
        </div>
      )}
    </nav>
  );
}
