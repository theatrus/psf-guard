import { useQuery } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { ArrowLeft, Check, TriangleAlert } from 'lucide-react';
import { apiClient } from '../../api/client';
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
  /** Rigs the plan names: a linked database, or a contribution. */
  rigs: Array<{ id: string; name: string }>;
  /** Open the activation preview from the line that says where it stands. */
  onActivation?: () => void;
  /** The activation bar is up and says it already. */
  hideActivation?: boolean;
}) {
  const { last, savedPlan, savedFraming, behind } = useActivationState(projectId);
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const framing = savedFraming.data?.draft;
  const objectives = savedPlan.data?.plan?.objectives ?? [];
  const activation = last.data
    ? behind.length > 0
      ? { ok: false, text: `${behind.join(' and ')} not activated` }
      : { ok: true, text: 'Active' }
    : last.isSuccess ? { ok: false, text: 'Not activated yet' } : null;
  return <div className="workspace-summary" aria-label="Plan summary" role="region">
    <div className="workspace-summary-head">
      <Link className="workspace-pill workspace-back" to={`/?${back}`}><ArrowLeft size={14} />Library</Link>
      <h2 className="workspace-summary-name">{projectName}</h2>
      {activation && !hideActivation && (onActivation
        ? <button type="button" className={`workspace-pill workspace-activation${activation.ok ? ' is-ok' : ' is-behind'}`} data-testid="summary-activation" title="Preview an activation, or push the last one again" onClick={onActivation}>{activation.ok ? <Check size={14} aria-hidden="true" /> : <TriangleAlert size={14} aria-hidden="true" />}{activation.text}</button>
        : <span className={`workspace-pill workspace-activation${activation.ok ? ' is-ok' : ' is-behind'}`} data-testid="summary-activation">{activation.text}</span>)}
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
          {rig.name}{gaps.length > 0 && <small> · {gaps.join(', ')}</small>}
        </span>;
      })}
      {rigs.length === 0 && <span className="workspace-pill">No rig in this plan yet</span>}
    </div>
  </div>;
}
