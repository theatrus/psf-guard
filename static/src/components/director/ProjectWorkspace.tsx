import { type KeyboardEvent as ReactKeyboardEvent, useCallback, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import FramingView from './FramingView';
import PlanEditor, { type PlanRigControls } from './PlanEditor';
import PlanRigs from './PlanRigs';
import PlanScheduling from './PlanScheduling';
import ObservingPreferences from './ObservingPreferences';
import ActivationBar from './ActivationBar';
import { useActivationDue } from './activationState';
import { DraftProvider, EditedMark, SaveBar } from './pageDrafts';
import WorkspaceSummary from './WorkspaceSummary';
import { usePageDrafts } from './pageDraftsState';
import type { FramingSeed } from './framingModel';
import { retryWhenBusy } from './retry';
import { withoutPlanningParams } from '../../hooks/useUrlState';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** The workspace's tabs, one job each. Every panel stays mounted, hidden
 *  when not shown, so an unsaved edit survives a switch; `draft` names the
 *  save-bar section a tab holds. */
const TABS = [
  { id: 'framing', label: 'Framing', drafts: ['framing'] },
  { id: 'exposures', label: 'Exposures', drafts: ['plan'] },
  { id: 'rigs', label: 'Rigs', drafts: ['scheduling'] },
  { id: 'priority', label: 'Priority and defaults', drafts: ['priority'] },
] as const;
type TabId = (typeof TABS)[number]['id'];
/** Tabs older links name. Activation is an action at the top now. */
const RENAMED: Record<string, TabId> = { plan: 'exposures', databases: 'rigs', activate: 'rigs' };

/** One global project: a summary of what it is and where it stands, with
 *  the save bar and activation at the top, then one tab at a time for the
 *  framing, the exposures, the rigs and their Target Scheduler projects,
 *  and the project priority. */
export default function ProjectWorkspace({ instanceId, projectId }: { instanceId: string; projectId: string }) {
  const [params, setParams] = useSearchParams();
  const { canWrite } = useAccess();
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, refetchOnMount: 'always' });
  const row = plans.data?.rows.find(entry => entry.project.id === projectId);
  const first = row?.links.find(link => link.source_row_id !== null);
  // The first linked catalog project seeds the framing; TS keeps RA in hours.
  const scheduler = useQuery({
    queryKey: ['db', first?.catalog_slug, 'project-scheduler', first?.source_row_id],
    queryFn: () => apiClient.getProjectScheduler(first!.catalog_slug, first!.source_row_id!),
    enabled: !!first, retry: false,
  });
  const seed: FramingSeed | null = useMemo(() => {
    const target = scheduler.data?.targets[0];
    if (!target) return null;
    return {
      name: target.name,
      center: { ra_degrees: ((target.ra_hours * 15) % 360 + 360) % 360, dec_degrees: target.dec_degrees },
      position_angle_degrees: Number.isFinite(target.rotation) ? ((target.rotation % 360) + 360) % 360 : 0,
    };
  }, [scheduler.data]);
  // The Library's Planning button names the database it came from; that
  // database's targets and exposures open at once.
  const cameFrom = params.get('db');
  const back = withoutPlanningParams(params.toString());
  // Framing, plan and priority edits wait in one draft for the save bar;
  // activation saves them before it reads the saved plan.
  const drafts = usePageDrafts();
  // The Rigs tab adds and drops rigs through the plan editor, which keeps
  // the plan; it reports back which rigs shoot it.
  const planControls = useRef<PlanRigControls | null>(null);
  const [planRigs, setPlanRigs] = useState<{ rigIds: string[]; objectives: number }>({ rigIds: [], objectives: 0 });
  const reportRigs = useCallback((next: { rigIds: string[]; objectives: number }) => setPlanRigs(next), []);
  const [activating, setActivating] = useState(false);
  const activationDue = useActivationDue(projectId, canWrite, drafts.unsaved.length);
  // The tab lives in the address, so a reload or a shared link opens it.
  // Without one, the page opens on the framing.
  const named = params.get('planTab');
  const asked = named && RENAMED[named] ? RENAMED[named] : named;
  const tab: TabId = TABS.some(entry => entry.id === asked) ? asked as TabId : 'framing';
  // The rig database editors read every database's rows, so they load
  // once their tab is first opened rather than with the page.
  const [visited, setVisited] = useState<Set<TabId>>(() => new Set([tab]));
  if (!visited.has(tab)) setVisited(current => new Set([...current, tab]));
  const chooseTab = (next: TabId) => setParams(current => { const copy = new URLSearchParams(current); copy.set('planTab', next); return copy; }, { replace: true });
  const onTabKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const index = TABS.findIndex(entry => entry.id === tab);
    const next = TABS[(index + step + TABS.length) % TABS.length];
    chooseTab(next.id);
    document.getElementById(`workspace-tab-${next.id}`)?.focus();
  };
  if (plans.isPending) return <p role="status">Loading project...</p>;
  if (plans.isError) return <div role="alert"><p>{message(plans.error)}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>;
  if (!row) return <p role="alert">Project not found. <Link to={`/?${back}`}>Back to the Library</Link></p>;
  const drafted = (ids: readonly string[]) => drafts.unsaved.some(section => ids.includes(section.id));
  // Every rig in the plan: a linked database, or a rig shooting it that
  // gets its project on activation.
  const summaryRigs = [...new Map([
    ...row.links.map(link => [link.rig.id, { id: link.rig.id, name: link.catalog_name }] as const),
    ...planRigs.rigIds.map(id => [id, { id, name: profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'A rig' }] as const),
  ]).values()];
  const panel = (id: TabId) => ({ role: 'tabpanel' as const, id: `workspace-panel-${id}`, 'aria-labelledby': `workspace-tab-${id}`, hidden: tab !== id, className: 'workspace-panel' });
  return <DraftProvider drafts={drafts}><section aria-label="Project planning" className="director-workspace">
    <div className="workspace-top">
      <SaveBar drafts={drafts} canWrite={canWrite} />
      <ActivationBar projectId={projectId} drafts={drafts} canWrite={canWrite} open={activating} onOpen={() => setActivating(true)} onClose={() => setActivating(false)} />
      <WorkspaceSummary projectId={projectId} projectName={row.project.name} back={back.toString()} rigs={summaryRigs} onActivation={() => setActivating(true)} hideActivation={activationDue} />
      <div className="workspace-tabs" role="tablist" aria-label="Plan sections" onKeyDown={onTabKey}>
        {TABS.map(entry => <button key={entry.id} type="button" role="tab" id={`workspace-tab-${entry.id}`} aria-selected={entry.id === tab} aria-controls={`workspace-panel-${entry.id}`}
          tabIndex={entry.id === tab ? 0 : -1} className={`workspace-tab${entry.id === tab ? ' active' : ''}`} onClick={() => chooseTab(entry.id)}>
          {entry.label}{drafted(entry.drafts) && <span className="workspace-tab-edited" aria-label="edited" title="Unsaved changes">●</span>}
        </button>)}
      </div>
    </div>
    <div {...panel('framing')}>
      {first && scheduler.isPending ? <p role="status">Loading targets...</p> : <FramingView projectId={projectId} seed={seed} preferredRigIds={summaryRigs.map(rig => rig.id)}
        shootingRigIds={planRigs.rigIds} onToggleRig={(id, on) => planControls.current?.setRig(id, on)}
        joinBlocked={canWrite && planRigs.objectives === 0 ? 'Add an objective on Exposures first' : undefined} />}
    </div>
    <div {...panel('exposures')}>
      <PlanEditor projectId={projectId} linkedRigIds={row.links.map(link => link.rig.id)} controls={planControls} onRigsChange={reportRigs} />
    </div>
    <div {...panel('rigs')}>
      <PlanRigs row={row} rows={plans.data?.rows ?? []} joined={planRigs.rigIds} objectives={planRigs.objectives} controls={planControls} cameFrom={cameFrom} load={visited.has('rigs')} />
      <h3 className="director-section-heading">Scheduling limits for every rig<EditedMark drafts={drafts} id="scheduling" /></h3>
      <PlanScheduling projectId={projectId} rigs={summaryRigs} />
    </div>
    <div {...panel('priority')}>
      <ObservingPreferences projectId={projectId} folded={false} rigs={row.links.map(link => ({ id: link.rig.id, name: link.catalog_name }))} projects={(plans.data?.rows ?? []).map(entry => entry.project)} />
    </div>
  </section></DraftProvider>;
}
