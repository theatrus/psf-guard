import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import type { FlatGroupLightCounts, FlatGroupUseView, FlatSession } from '../../api/types';
import type { GroupEdge, GroupEntry } from './flatGroupBrowse';
import { splitShortReason } from './flatGroupBrowse';
import { GroupMarks } from './FlatGroupList';
import { calibrationMasterHref, lightsGridHref } from './calibrationLinks';
import { flatGroupFilterValue } from '../../utils/flatGroupFilter';
import { editFlatLimits } from './flatLimitsLink';
import {
  eventSeenIn,
  eventSideLabel,
  limitPercent,
  nightRange,
  percent,
  plural,
  timeLabel,
  windowLabel,
} from './flatGroupLabels';

/** Masters listed before Show all: the newest, since a library rebuilds
 *  a group's master as its settings change. */
const MASTERS_SHOWN = 4;

interface FlatGroupDetailProps {
  dbId: string;
  entry: GroupEntry;
  /** What the groups feed, once read. */
  use: FlatGroupUseView | undefined;
  useError: Error | null;
  /** The project in scope. */
  scope?: { projectId: number; name: string };
  canManage: boolean;
  /** The pair of sessions compared now. */
  open: { earlier: string; later: string } | null;
  onCompare: (earlier: string, later: string, opener: HTMLElement) => void;
  /** Leave for one of its sessions in the session list. */
  onOpenSession: (id: string) => void;
  /** Bring the strip above to one of its nights. */
  onShowNight: (id: string) => void;
  /** The night of the frame in the viewer, by its session. */
  shownSession: string | null;
  /** The session being found in the list, while it is. */
  opening: string | null;
}


/** The lights by how their flats were chosen, beyond those on their own side. */
function lightsBreakdown(lights: FlatGroupLightCounts): string[] {
  return [
    lights.undetermined > 0 ? `${lights.undetermined} undetermined` : null,
    lights.other_angle > 0 ? `${lights.other_angle} from another angle` : null,
    lights.corrected > 0 ? `${lights.corrected} carried across a change` : null,
    lights.no_same_side > 0 ? `${lights.no_same_side} from the other side of a change` : null,
  ].filter((part): part is string => part !== null);
}

function EdgeDetail({
  side,
  edge,
  open,
  onCompare,
}: {
  side: 'newer' | 'older';
  edge: GroupEdge;
  open: FlatGroupDetailProps['open'];
  onCompare: FlatGroupDetailProps['onCompare'];
}) {
  const pressed = open?.earlier === edge.earlier && open.later === edge.later;
  const { split, event } = edge;
  const heading = event ? 'Change' : 'Split';
  return (
    <li className="flat-group-edge-detail" data-testid={`flat-group-${side}-edge`}>
      <div className="flat-mark-text">
        <strong>
          {side === 'newer' ? 'After it' : 'Before it'} · {heading}
          {split ? ` · ${splitShortReason(split)}` : ''}
        </strong>
        {split ? <span>{split.reason}</span> : <span>{event!.what}</span>}
        {event && (
          <span className="calibration-muted">
            {eventSideLabel(event)} · seen in {eventSeenIn(event)}
          </span>
        )}
        <span className="calibration-muted">
          {split ? `${nightRange(split.before_night, split.after_night)} · ` : ''}
          {windowLabel(split?.start ?? event!.start, split?.end ?? event!.end)}
        </span>
      </div>
      <button
        type="button"
        className="tool-button flat-mark-compare"
        data-testid="flat-group-compare"
        aria-pressed={pressed}
        aria-label={`Compare across the ${side === 'newer' ? 'newer' : 'older'} edge`}
        onClick={(click) => onCompare(edge.earlier, edge.later, click.currentTarget)}
      >
        Compare
      </button>
    </li>
  );
}

/** How much of a limit a difference takes, as a bar. */
function LimitBar({ value, limit }: { value: number; limit: number }) {
  const share = limit > 0 ? Math.min(1, value / limit) : 0;
  return (
    <span className="flat-group-bar" aria-hidden="true">
      <span style={{ width: `${Math.round(share * 100)}%` }} />
    </span>
  );
}

/** A member session's button: its night, flats and how it measured against the night before. */
function MemberCard({
  session,
  id,
  shown,
  opening,
  onShow,
  onOpen,
}: {
  session: FlatSession;
  id: string;
  /** Its night is the one in the viewer. */
  shown: boolean;
  opening: boolean;
  onShow: (id: string) => void;
  onOpen: (id: string) => void;
}) {
  return (
    <div className="flat-group-member-card">
      <button type="button" className="flat-group-member" data-testid="flat-group-member"
        aria-current={shown || undefined} onClick={() => onShow(id)}
        title="Show this night's flats in the strip above">
        <strong>{session.night}</strong>
        <span>{plural(session.frames, 'flat')}</span>
        {session.noise_pct !== null && <span className="calibration-muted">noise {percent(session.noise_pct)}</span>}
      </button>
      <button type="button" className="calibration-link-button flat-group-open-session"
        data-testid="flat-group-open-session" aria-busy={opening || undefined}
        aria-label={`Open the session of ${session.night}`} onClick={() => onOpen(id)}>
        Open session
      </button>
    </div>
  );
}

/** A flat group's name above its frames, as a session's title is. */
export function FlatGroupHeader({ entry, scopeName }: { entry: GroupEntry; scopeName?: string }) {
  const { rig, group } = entry;
  return (
    <div className="calibration-session-bar flat-group-detail-head" data-flat-tone={entry.tone}
      data-testid="flat-group-header">
      <div>
        <h2 id="flat-group-detail-title">
          <span className="flat-group-swatch" aria-hidden="true" />
          Group {entry.number} of {entry.groups} · {entry.familyLabel}
        </h2>
        <p className="calibration-muted">
          {[rig.name, nightRange(group.first_night, group.last_night), plural(group.sessions.length, 'night'), plural(group.frames, 'flat')].join(' · ')}
        </p>
        <GroupMarks entry={entry} scopeName={scopeName} />
      </div>
    </div>
  );
}

/**
 * One flat group in Frames' main column: the nights it holds, why they
 * belong together against the rig's limits, the split or change on each
 * side with a comparison across it, and what it feeds.
 */
export default function FlatGroupDetail({
  dbId,
  entry,
  use,
  useError,
  scope,
  canManage,
  open,
  onCompare,
  onOpenSession,
  onShowNight,
  shownSession,
  opening,
}: FlatGroupDetailProps) {
  const { search } = useLocation();
  const [allMasters, setAllMasters] = useState(false);
  const { rig, group } = entry;
  const members = [...group.sessions].reverse();
  const pairs = group.pairs ?? [];
  const widest = group.widest ?? null;
  const fed = use?.groups.find((candidate) => candidate.key === entry.key);
  // The counts are being worked out again: the last ones stay, said to be
  // updating, and none is shown as fact until the first are in.
  const updating = !!use && (use.refreshing || !use.computed);
  const known = !!use?.computed;
  const projects = [...(fed?.projects ?? [])].sort((a, b) =>
    Number(b.project_id === scope?.projectId) - Number(a.project_id === scope?.projectId)
    || b.lights.total - a.lights.total);
  const breakdown = fed ? lightsBreakdown(fed.lights) : [];

  return (
    <section className="flat-group-detail" aria-labelledby="flat-group-detail-title" data-testid="flat-group-detail">
      <section className="flat-group-section" aria-labelledby="flat-group-members">
        <h3 id="flat-group-members">Members</h3>
        <ul className="flat-group-members" data-flat-tone={entry.tone}>
          {members.flatMap((session) => {
            const ids = session.sessions ?? [];
            return ids.length === 0
              ? [<li key={session.response_key} className="flat-group-member is-static"><strong>{session.night}</strong><span>{plural(session.frames, 'flat')}</span></li>]
              : ids.map((id) => (
                  <li key={id}>
                    <MemberCard session={session} id={id} shown={shownSession === id} opening={opening === id}
                      onShow={onShowNight} onOpen={onOpenSession} />
                  </li>
                ));
          })}
          {entry.unmeasured.flatMap((session) => (session.sessions ?? []).map((id) => (
            <li key={id}>
              <button type="button" className="flat-group-member is-unmeasured" data-testid="flat-group-unmeasured"
                aria-busy={opening === id || undefined} onClick={() => onOpenSession(id)}
                title="Open this session's frames">
                <strong>{session.night}</strong>
                <span>{plural(session.frames, 'flat')}</span>
                <span className="calibration-muted">
                  {session.reason === 'not measured yet' ? 'not measured yet' : `not measured: ${session.reason}`}
                </span>
              </button>
            </li>
          )))}
        </ul>
      </section>

      <section className="flat-group-section" aria-labelledby="flat-group-why">
        <h3 id="flat-group-why">Why they belong together</h3>
        <p className="calibration-muted">
          {rig.measured
            ? `${rig.name} splits flats at ${limitPercent(rig.dust_limit_pct)} dust and ${limitPercent(rig.pattern_limit_pct)} light pattern${(rig.own_limits ?? []).length > 0 ? ', its own limits' : ''}.`
            : `${rig.name} groups its flats by time, not by measurement.`}
          {canManage && (
            <>
              {' '}
              <button type="button" className="calibration-link-button" data-testid="flat-group-edit-limits"
                aria-label={`Edit limits for ${rig.name}`} onClick={() => editFlatLimits(rig)}>
                Edit limits
              </button>
            </>
          )}
        </p>
        {group.sessions.length === 1 ? (
          <p className="calibration-muted">One night: nothing within it to compare.</p>
        ) : !widest || pairs.length === 0 ? (
          <p className="calibration-muted">Its nights were grouped without a measurement between them.</p>
        ) : (
          <>
            <p data-testid="flat-group-widest">
              At most {percent(widest.dust_pct)} dust (limit {limitPercent(rig.dust_limit_pct)}) and {percent(widest.broad_pct)} light
              pattern (limit {limitPercent(rig.pattern_limit_pct)}) between any two of its nights,
              over {plural(widest.compared, 'pair')} measured. Below, each night against the next, and the pairs
              furthest apart.
            </p>
            <table className="flat-group-pairs" data-testid="flat-group-pairs">
              <thead>
                <tr><th scope="col">Nights</th><th scope="col">Dust</th><th scope="col">Pattern</th></tr>
              </thead>
              <tbody>
                {pairs.map((pair) => (
                  <tr key={`${pair.earlier_response}:${pair.later_response}`}>
                    <th scope="row">{pair.later_night} ÷ {pair.earlier_night}</th>
                    <td>
                      <LimitBar value={pair.dust_pct} limit={rig.dust_limit_pct} />
                      {percent(pair.dust_pct)}
                    </td>
                    <td>
                      <LimitBar value={pair.broad_pct} limit={rig.pattern_limit_pct} />
                      {percent(pair.broad_pct)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </section>

      <section className="flat-group-section" aria-labelledby="flat-group-edges">
        <h3 id="flat-group-edges">Its edges</h3>
        <ul className="flat-group-edges">
          {entry.newer
            ? <EdgeDetail side="newer" edge={entry.newer} open={open} onCompare={onCompare} />
            : <li className="calibration-muted">Nothing newer: a light shot now takes these flats.</li>}
          {entry.older
            ? <EdgeDetail side="older" edge={entry.older} open={open} onCompare={onCompare} />
            : <li className="calibration-muted">The oldest group of {entry.familyLabel}.</li>}
        </ul>
      </section>

      <section className="flat-group-section" aria-labelledby="flat-group-feeds" data-testid="flat-group-feeds">
        <h3 id="flat-group-feeds">What it feeds</h3>
        {useError ? (
          <p className="calibration-error">Could not work out what it feeds: {useError.message}</p>
        ) : !known && use?.error ? (
          <p className="calibration-error" data-testid="flat-group-use-error">
            Could not work out what it feeds: {use.error}. Trying again in a while.
          </p>
        ) : !use || !known ? (
          <p className="calibration-muted" role="status">Working out which masters and lights use it…</p>
        ) : (
          <>
            {use.error && (
              <p className="calibration-error" data-testid="flat-group-use-error">
                These counts may be out of date: working out newer ones failed ({use.error}). Trying again in a while.
              </p>
            )}
            {updating && (
              <p className="calibration-muted" role="status" data-testid="flat-group-updating">
                Updating… these are the last counts while newer ones are worked out.
              </p>
            )}
            {fed && fed.masters.length > 0 ? (
              <ul className="flat-group-masters" aria-label="Flat masters built from it">
                {(allMasters ? fed.masters : fed.masters.slice(0, MASTERS_SHOWN)).map((master) => (
                  <li key={master.id}>
                    <Link to={calibrationMasterHref(search, dbId, master.id)} title={master.label}>
                      Master built {timeLabel(master.created_at)}
                    </Link>
                    <span className="calibration-muted">
                      {' · '}{plural(master.sources, 'flat')}
                      {master.in_group < master.sources ? `, ${master.in_group} of them from this group` : ''}
                    </span>
                  </li>
                ))}
                {fed.masters.length > MASTERS_SHOWN && (
                  <li>
                    <button type="button" className="calibration-link-button" aria-expanded={allMasters}
                      onClick={() => setAllMasters((shown) => !shown)}>
                      {allMasters ? 'Show the newest' : `Show all ${fed.masters.length}`}
                    </button>
                  </li>
                )}
              </ul>
            ) : !updating && (
              <p className="calibration-muted">No flat master has been built from it yet.</p>
            )}
            {fed && fed.lights.total > 0 ? (
              <>
                <p data-testid="flat-group-lights">
                  {plural(fed.lights.total, 'light')} take its flats
                  {breakdown.length > 0 ? `: ${fed.lights.own} on their own side, ${breakdown.join(', ')}` : ''}.
                </p>
                <ul className="flat-group-projects">
                  {projects.map((project) => {
                    const name = project.name ?? `Project ${project.project_id}`;
                    const flagged = project.lights.undetermined + project.lights.no_same_side;
                    // Exactly these lights: the grid's flat-group filter.
                    const filter = (flaggedOnly: boolean) => flatGroupFilterValue({ dbId, key: entry.key, flagged: flaggedOnly });
                    return (
                      <li key={project.project_id}>
                        <Link to={lightsGridHref(search, dbId, project.project_id, { flatGroup: filter(false) })}
                          data-testid="flat-group-lights-link">
                          {name}: {plural(project.lights.total, 'light')}
                        </Link>
                        {lightsBreakdown(project.lights).length > 0 && (
                          <span className="calibration-muted"> · {lightsBreakdown(project.lights).join(', ')}</span>
                        )}
                        {flagged > 0 && (
                          <>
                            {' · '}
                            <Link to={lightsGridHref(search, dbId, project.project_id, { flatGroup: filter(true) })}
                              data-testid="flat-group-flagged-link">
                              the {flagged} flagged
                            </Link>
                          </>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </>
            ) : !updating && (
              <p className="calibration-muted">No light takes its flats{use.pending > 0 ? ' yet' : ''}.</p>
            )}
            {use.pending > 0 && (
              <p className="calibration-muted">
                {plural(use.pending, 'light')} not read by the calibration check yet; these counts grow as it reads them.
              </p>
            )}
            {use.unread > 0 && (
              <p className="calibration-muted" data-testid="flat-group-unread">
                {plural(use.unread, 'light')} in the catalog could not be read or found, so are not counted.
              </p>
            )}
          </>
        )}
      </section>
    </section>
  );
}
