import { Fragment } from 'react';
import type { CalibrationSession } from '../../api/types';
import { nightLabel, plural, sessionDetails, sessionTitle } from './calibrationLabels';
import { FlatGroupBadge, FlatMarkRow } from './FlatGroupMarks';
import type { FlatGroupIndex, FlatMark } from './flatGroupIndex';

/** Flat groups drawn into the list: each flat's group, and the splits and
 *  changes between them. */
export interface SessionListFlatGroups {
  index: FlatGroupIndex;
  /** The pair of flat sessions compared now. */
  open: { earlier: string; later: string } | null;
  onCompare: (earlier: string, later: string, opener: HTMLElement) => void;
}

interface CalibrationSessionListProps {
  sessions: CalibrationSession[];
  selected: string | null;
  showRig: boolean;
  page: number;
  perPage: number;
  total: number;
  onSelect: (id: string) => void;
  onPage: (page: number) => void;
  flatGroups?: SessionListFlatGroups;
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
  flatGroups,
}: CalibrationSessionListProps) {
  const first = page * perPage + 1;
  const last = Math.min(total, (page + 1) * perPage);
  const marks = flatGroups?.index.marks(sessions, page);
  const nightOf = (key: string) => flatGroups?.index.response(key)?.night;
  return (
    <nav className="calibration-sessions" aria-label="Sessions">
      <ul>
        {sessions.map((session, index) => {
          const place = session.kind === 'flat' ? flatGroups?.index.place(session.id) : undefined;
          const unmeasured = session.kind === 'flat' && !place ? flatGroups?.index.unmeasured(session.id) : undefined;
          return (
            <Fragment key={session.id}>
              {flatGroups && marks?.get(index)?.map((mark) => (
                <FlatMarkRow key={mark.key} mark={mark} nightOf={nightOf}
                  open={isOpen(flatGroups.open, mark)} onCompare={flatGroups.onCompare} />
              ))}
              <li data-flat-tone={place?.tone}>
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
                  {(place || unmeasured) && <FlatGroupBadge place={place} unmeasured={unmeasured} />}
                </button>
              </li>
            </Fragment>
          );
        })}
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

function isOpen(open: SessionListFlatGroups['open'], mark: FlatMark): boolean {
  if (!open) return false;
  const pair = mark.kind === 'event' ? mark.event : mark.split;
  return pair.before_response === open.earlier && pair.after_response === open.later;
}
