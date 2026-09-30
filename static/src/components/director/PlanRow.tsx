import { Link } from 'react-router-dom';
import { Pencil } from 'lucide-react';
import type { DirectorPlanRow } from '../../api/directorTypes';
import { DbPill, FamilyPill, ProjectRow, StatePill } from '../projectPills';
import { stage, stageShort } from './planCardModel';

interface PlanRowProps {
  row: DirectorPlanRow;
  href: string;
  canWrite: boolean;
  editing: boolean;
  onRename: () => void;
}

/** A plan with nothing captured yet, as a Library row: its name opens the
 *  workspace, then how far planning has come and each rig with its project
 *  state there, or that no database takes it yet. */
export default function PlanRow({ row, href, canWrite, editing, onRename }: PlanRowProps) {
  const name = row.project.name;
  return <ProjectRow className="director-plan-row" testId="plan-row">
    <Link className="library-name plan-row-name" to={href} aria-label={`Open ${name}`}>{name}</Link>
    {row.activation
      ? <span className="director-stage-badge">Active on {row.activation.rigs} rig{row.activation.rigs === 1 ? '' : 's'}</span>
      : (row.framing || row.plan) && <span className="library-pill library-pill-stage" title={stage(row)}>{stageShort(row)}</span>}
    {row.links.length > 1 && <FamilyPill rigs={row.links.length} />}
    {row.links.length === 0
      ? <span className="library-pill library-pill-stage">Not linked to any database</span>
      : row.links.map(link => <span key={`${link.catalog_slug}:${link.source_project_guid}`} className="plan-row-rig">
          <DbPill name={link.catalog_name} title={`Rig ${link.rig.name}`} /><StatePill state={link.source_state} />
        </span>)}
    <span className="library-pill">No frames yet</span>
    {canWrite && <button type="button" className="library-planning" disabled={editing} title={`Rename ${name}`} aria-label={`Rename ${name}`} onClick={onRename}><Pencil size={14} /></button>}
  </ProjectRow>;
}
