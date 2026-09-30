import { Link } from 'react-router-dom';
import { Images, LibraryBig, Pencil } from 'lucide-react';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { DatesPill, DbPill, FamilyPill, FoldButton, GradingPill, ProgressPill, ProjectRow, StatePill } from '../projectPills';
import { ago, stateLabel } from '../libraryFamilies';
import { stage, stageShort, sumFrames } from './planCardModel';

/** How a row may change a rig's project: nothing, or the Target Scheduler
 *  state, written straight into that rig's database. */
export interface RigActions {
  canEditState: boolean;
  onStateChange: (link: DirectorPlanLink, state: number) => void;
}

interface PlanRowProps {
  row: DirectorPlanRow;
  href: string;
  nowMs: number;
  /** Opens the row into its full card. */
  onOpen: () => void;
  canWrite: boolean;
  editing: boolean;
  onRename: () => void;
  actions: RigActions;
}

const STATES = [0, 1, 2, 3];

/** The project's Target Scheduler state on one rig: a pill, or a select
 *  when this server may write the rig's database. */
export function RigState({ link, actions }: { link: DirectorPlanLink; actions: RigActions }) {
  const state = link.source_state;
  if (!actions.canEditState || link.source_row_id === null || state === null || state === undefined) return <StatePill state={state} />;
  const name = link.source_name ?? link.catalog_name;
  return <select className={`library-pill library-pill-state is-state-${state}`} value={state} aria-label={`State of ${name} in ${link.catalog_name}`}
    title="Target Scheduler project state on this rig" onChange={event => actions.onStateChange(link, Number(event.target.value))}>
    {STATES.map(value => <option key={value} value={value}>{stateLabel(value)}</option>)}
  </select>;
}

/** Where else this rig's project lives: the Library card and the image grid. */
export function RigLinks({ link }: { link: DirectorPlanLink }) {
  if (link.source_row_id === null) return null;
  const name = link.source_name ?? link.catalog_name;
  const scope = `db=${encodeURIComponent(link.catalog_slug)}&project=${link.source_row_id}`;
  return <>
    <Link className="library-planning" to={`/?${scope}`} title={`Show ${name} in the Library`} aria-label={`Show ${name} in the Library`}><LibraryBig size={14} /></Link>
    <Link className="library-planning" to={`/grid?${scope}`} title={`Show ${name} images`} aria-label={`Show ${name} images`}><Images size={14} /></Link>
  </>;
}

/** One rig's share of a plan, in the Library's pills: its database, the
 *  project's state there, accepted against desired, the grading split and
 *  when it last shot, then where else to open it. */
function LinkPills({ link, nowMs, actions }: { link: DirectorPlanLink; nowMs: number; actions: RigActions }) {
  const sum = sumFrames(link.targets);
  const pending = Math.max(0, sum.acquired - sum.accepted - sum.rejected);
  return <>
    <DbPill name={link.catalog_name} title={`Rig ${link.rig.name}`} />
    <RigState link={link} actions={actions} />
    {link.source_row_id === null && <span className="library-pill files-missing">project row missing</span>}
    <ProgressPill accepted={sum.accepted} desired={sum.desired} totalImages={sum.acquired} title={link.targets.length === 0 ? 'No target yet' : undefined} />
    <GradingPill accepted={sum.accepted} rejected={sum.rejected} pending={pending} />
    <DatesPill earliest={link.earliest_capture_s} latest={link.latest_capture_s} nowMs={nowMs} />
    <RigLinks link={link} />
  </>;
}

/** A plan as the Library shows a project: one row when one rig shoots it,
 *  an outer pill with a row per rig when several do. The arrow opens the
 *  full card; the name opens the workspace. */
export default function PlanRow({ row, href, nowMs, onOpen, canWrite, editing, onRename, actions }: PlanRowProps) {
  const name = row.project.name;
  const rename = canWrite && <button type="button" className="library-planning" disabled={editing} title={`Rename ${name}`} aria-label={`Rename ${name}`} onClick={onRename}><Pencil size={14} /></button>;
  const badge = row.activation
    ? <span className="director-stage-badge">Active on {row.activation.rigs} rig{row.activation.rigs === 1 ? '' : 's'}</span>
    : row.links.length > 0 && <span className="library-pill library-pill-stage" title={stage(row)}>{stageShort(row)}</span>;
  if (row.links.length > 1) {
    const total = sumFrames(row.links.flatMap(link => link.targets));
    const latest = row.links.map(link => link.latest_capture_s ?? 0).reduce((a, b) => Math.max(a, b), 0);
    const last = ago(latest > 0 ? latest : null, nowMs);
    return <div className="library-family is-compact" data-testid="plan-family">
      <div className="library-family-head">
        <FoldButton open={false} name={name} onClick={onOpen} />
        <FamilyPill rigs={row.links.length} />
        <Link className="library-family-name plan-row-name" to={href} aria-label={`Open ${name}`}>{name}</Link>
        {badge}
        <DbPill name={row.links.map(link => link.catalog_name).join(' · ')} />
        <ProgressPill accepted={total.accepted} desired={total.desired} totalImages={total.acquired} title={`${total.accepted} of ${total.desired} desired frames accepted across the rigs`} />
        {last && <span className="library-pill library-pill-dates">{last}</span>}
        {rename}
      </div>
      <div className="library-family-members">
        {row.links.map(link => (
          <ProjectRow key={`${link.catalog_slug}:${link.source_project_guid}`} className="director-plan-row" testId="plan-row">
            <span className="library-name is-static">{link.source_name ?? name}</span>
            <LinkPills link={link} nowMs={nowMs} actions={actions} />
          </ProjectRow>
        ))}
      </div>
    </div>;
  }
  const link = row.links[0];
  return <ProjectRow className={`director-plan-row${row.activation ? ' is-activated' : ''}`} testId="plan-row">
    <FoldButton open={false} name={name} onClick={onOpen} />
    <Link className="library-name plan-row-name" to={href} aria-label={`Open ${name}`}>{name}</Link>
    {badge}
    {link ? <LinkPills link={link} nowMs={nowMs} actions={actions} /> : <span className="library-pill library-pill-stage">{stage(row)}</span>}
    {rename}
  </ProjectRow>;
}
