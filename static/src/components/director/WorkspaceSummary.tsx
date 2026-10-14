import { useQuery } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { ArrowLeft, Check, Send, TriangleAlert } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import type { DirectorObjective, DirectorRigProfileSummary } from '../../api/directorTypes';
import { useActivationState } from './activationState';
import { formatDec, formatRaHours } from './framingModel';
import { KNOWN_BANDPASSES } from './planModel';
import { retryWhenBusy } from './retry';

/** What a rig still lacks for planning: the parts of its profile the
 *  footprint, the visibility and the exposures are worked out from. */
function rigGaps(rig: DirectorRigProfileSummary | undefined): string[] {
  if (!rig?.profile) return ['no rig profile'];
  const gaps: string[] = [];
  if (!rig.profile.optics && !rig.field_of_view) gaps.push('no optics');
  // The rig's own location, else its planning site's.
  if (!(rig.site ? rig.site.location : rig.profile.site)) gaps.push('no site');
  return gaps;
}

const bandpassName = (id: string) => KNOWN_BANDPASSES.find(entry => entry.id === id)?.name ?? id;

function goalText(objective: DirectorObjective): string {
  const { kind, value } = objective.goal;
  return `${bandpassName(objective.bandpass_id)} · ${kind === 'frames' ? `${value} frames per rig` : `${value} h`}`;
}

/** The workspace's summary, once at the top of every tab: the project,
 *  where it points, what it asks for, each rig and whether it is ready, and
 *  whether the rig databases have what is saved. Each fact lives here so
 *  the tabs need not repeat it. */
export default function WorkspaceSummary({ projectId, projectName, back, rigs, onActivation, hideActivation = false }: {
  projectId: string;
  projectName: string;
  /** The Library address to go back to. */
  back: string;
  /** Rigs the plan names: a linked database, or a contribution. `off`: a
   *  linked rig that shoots none of it. */
  rigs: Array<{ id: string; name: string; off?: boolean }>;
  /** Open the activation preview; the summary offers it beside where the
   *  plan stands. */
  onActivation?: () => void;
  /** The activation bar is up and says it already. */
  hideActivation?: boolean;
}) {
  const { last, savedPlan, savedFraming, behind, notSynced, matches, checking, unchecked } = useActivationState(projectId);
  const status = useDirectorStatus();
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const framing = savedFraming.data?.draft;
  const objectives = savedPlan.data?.plan?.objectives ?? [];
  // Rows that already match count as synced, recorded or not. Not "Active":
  // that is a Target Scheduler project state, which the project in each
  // database keeps whatever this says. The pill stays short; why is in its
  // tooltip. A current record answers at once; where it cannot, the rows
  // are read first, rather than say one thing and then another.
  const synced = { tone: 'ok', text: 'Synced', title: 'The rigs hold the saved plan' } as const;
  const activation: { tone?: 'ok' | 'behind'; text: string; title?: string } | null = checking ? { text: 'Checking the rigs…' }
    : matches ? synced
    : last.data
      ? behind.length > 0 ? { tone: 'behind', text: 'Not synced', title: notSynced }
        : unchecked ? { tone: 'behind', ...unchecked }
        : synced
      : last.isSuccess ? { tone: 'behind', text: 'Not synced', title: notSynced } : null;
  return <div className="workspace-summary" aria-label="Plan summary" role="region">
    <div className="workspace-summary-head">
      <Link className="workspace-pill workspace-back" to={`/?${back}`}><ArrowLeft size={14} />Library</Link>
      <h2 className="workspace-summary-name">{projectName}</h2>
      {status.data && !status.data.database_management && <span className="workspace-pill" title="This server does not write to rig databases" data-testid="summary-drafts-only">Drafts only</span>}
      {/* Where the plan stands is a pill; the action beside it is a button. */}
      {activation && !hideActivation && <span className="workspace-activation">
        <span className={`workspace-pill${activation.tone ? ` is-${activation.tone}` : ''}`} data-testid="summary-activation" title={activation.title}>
          {activation.tone === 'ok' && <Check size={14} aria-hidden="true" />}
          {activation.tone === 'behind' && <TriangleAlert size={14} aria-hidden="true" />}
          {activation.text}
        </span>
        {onActivation && <button type="button" className="workspace-activate" title="Preview an activation, or push the last one again" onClick={onActivation}><Send size={14} aria-hidden="true" />Activate…</button>}
      </span>}
    </div>
    <div className="workspace-summary-line">
      {framing
        ? <span className="workspace-pill is-strong" data-testid="summary-target">{framing.target_name} <small>{formatRaHours(framing.center.ra_degrees)} {formatDec(framing.center.dec_degrees)}</small></span>
        : savedFraming.isSuccess && <span className="workspace-pill">No framing saved yet</span>}
      {objectives.length > 0
        ? <span className="workspace-pills" data-testid="summary-goals">{objectives.map(objective => <span key={objective.id} className="workspace-pill">{goalText(objective)}</span>)}</span>
        : savedPlan.isSuccess && <span className="workspace-pill">No objectives yet</span>}
      {rigs.map(rig => {
        const gaps = profiles.data ? rigGaps(profiles.data.find(entry => entry.rig.id === rig.id)) : [];
        return <span key={rig.id} className={`workspace-pill workspace-rig${gaps.length ? ' is-missing' : ' is-ok'}`}
          title={gaps.length ? `${rig.name}: ${gaps.join(', ')}. Set it up under Settings → Rigs.` : `${rig.name} is set up`}>
          {gaps.length ? <TriangleAlert size={14} aria-hidden="true" /> : <Check size={14} aria-hidden="true" />}
          {rig.name}{gaps.length > 0 && <small> · {gaps.join(', ')}</small>}{rig.off && <small> · off</small>}
        </span>;
      })}
      {rigs.length === 0 && <span className="workspace-pill">No rig in this plan yet</span>}
    </div>
  </div>;
}
