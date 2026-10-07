import { type KeyboardEvent as ReactKeyboardEvent, useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import FramingView from './FramingView';
import PlanEditor, { type PlanRigControls, type PlanRigState } from './PlanEditor';
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
/** The address names the plan; going to another plan leaves this page. */
const PAGE_KEYS = ['plan'];

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
  const measureTop = useWorkspaceRoom();
  const planControls = useRef<PlanRigControls | null>(null);
  const [planRigs, setPlanRigs] = useState<PlanRigState>({ rigIds: [], objectives: 0, ready: false });
  const reportRigs = useCallback((next: PlanRigState) => setPlanRigs(next), []);
  // Why a rig switched on in the framing did not join, until the next switch.
  const [framingNote, setFramingNote] = useState('');
  const [activating, setActivating] = useState(false);
  const section = useRef<HTMLElement>(null);
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
  // The framing waits for the first database's targets once. After that it
  // stays mounted, so an attach or detach that changes the first database
  // cannot take unsaved framing edits with it.
  const waitingForTargets = !!first && scheduler.isPending;
  const [framingShown, setFramingShown] = useState(false);
  if (!framingShown && !waitingForTargets && plans.isSuccess) setFramingShown(true);
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
    ...row.links.map(link => [link.rig.id, { id: link.rig.id, name: link.catalog_name, off: planRigs.ready && !planRigs.rigIds.includes(link.rig.id) }] as const),
    ...planRigs.rigIds.map(id => [id, { id, name: profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'A rig', off: false }] as const),
  ]).values()];
  const panel = (id: TabId) => ({ role: 'tabpanel' as const, id: `workspace-panel-${id}`, 'aria-labelledby': `workspace-tab-${id}`, hidden: tab !== id, className: 'workspace-panel' });
  const closeActivation = () => {
    setActivating(false);
    // An Apply takes the Activate button the dialog would return focus to
    // away; the summary's activation line, or the open tab, takes it.
    window.setTimeout(() => {
      if (document.activeElement && document.activeElement !== document.body) return;
      (section.current?.querySelector<HTMLElement>('[data-testid="summary-activation"]') ?? document.getElementById(`workspace-tab-${tab}`))?.focus();
    });
  };
  const toggleFromFraming = (id: string, on: boolean) => {
    const change = planControls.current?.setRig(id, on) ?? { done: false, note: 'plan still loading' };
    const name = profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'The rig';
    setFramingNote(change.done ? '' : `${name}: ${change.note ?? 'not added'}`);
    return change.done;
  };
  const joinBlocked = !canWrite ? undefined : !planRigs.ready ? 'Plan still loading' : planRigs.objectives === 0 ? 'Add an objective on Exposures first' : undefined;
  return <DraftProvider drafts={drafts}><section aria-label="Project planning" className="director-workspace" ref={section}>
    <div className="workspace-top" ref={measureTop}>
      <SaveBar drafts={drafts} canWrite={canWrite} pageKeys={PAGE_KEYS} />
      <ActivationBar projectId={projectId} drafts={drafts} canWrite={canWrite} open={activating} onOpen={() => setActivating(true)} onClose={closeActivation} />
      <WorkspaceSummary projectId={projectId} projectName={row.project.name} back={back.toString()} rigs={summaryRigs} onActivation={() => setActivating(true)} hideActivation={activationDue} />
      <div className="workspace-tabs" role="tablist" aria-label="Plan sections" onKeyDown={onTabKey}>
        {TABS.map(entry => <button key={entry.id} type="button" role="tab" id={`workspace-tab-${entry.id}`} aria-selected={entry.id === tab} aria-controls={`workspace-panel-${entry.id}`}
          tabIndex={entry.id === tab ? 0 : -1} className={`workspace-tab${entry.id === tab ? ' active' : ''}`} onClick={() => chooseTab(entry.id)}>
          {entry.label}{drafted(entry.drafts) && <><span className="workspace-tab-edited" aria-hidden="true" title="Unsaved changes">●</span><span className="workspace-tab-edited-text">, edited</span></>}
        </button>)}
      </div>
      {tab === 'framing' && framingNote && <p className="director-error workspace-note" role="alert">{framingNote}</p>}
    </div>
    <div {...panel('framing')}>
      {!framingShown ? <p role="status">Loading targets...</p> : <FramingView projectId={projectId} seed={seed} preferredRigIds={summaryRigs.map(rig => rig.id)}
        shootingRigIds={planRigs.rigIds} onToggleRig={toggleFromFraming} joinBlocked={joinBlocked} />}
    </div>
    <div {...panel('exposures')}>
      <PlanEditor projectId={projectId} linkedRigIds={row.links.map(link => link.rig.id)} controls={planControls} onRigsChange={reportRigs} />
    </div>
    <div {...panel('rigs')}>
      <PlanRigs row={row} rows={plans.data?.rows ?? []} joined={planRigs.rigIds} objectives={planRigs.objectives} ready={planRigs.ready} controls={planControls} cameFrom={cameFrom} load={visited.has('rigs')} />
      <h3 className="director-section-heading">Scheduling limits for every rig<EditedMark drafts={drafts} id="scheduling" /></h3>
      <PlanScheduling projectId={projectId} rigs={summaryRigs} />
    </div>
    <div {...panel('priority')}>
      <ObservingPreferences projectId={projectId} folded={false} rigs={row.links.map(link => ({ id: link.rig.id, name: link.catalog_name }))} projects={(plans.data?.rows ?? []).map(entry => entry.project)} />
    </div>
  </section></DraftProvider>;
}

/**
 * Measure the sticky top and the room the scrolling column leaves below it,
 * into --workspace-top and --workspace-room on the workspace, so the framing
 * sky and its controls fill the window however tall the bars are.
 */
function useWorkspaceRoom() {
  const [top, setTop] = useState<HTMLElement | null>(null);
  // Measured before paint, so the first frame already has the room.
  useLayoutEffect(() => {
    const workspace = top?.parentElement;
    if (!top || !workspace) return;
    const scroller = top.closest<HTMLElement>('.app-main');
    const measure = () => {
      const view = scroller?.clientHeight || window.innerHeight;
      const height = top.offsetHeight;
      workspace.style.setProperty('--workspace-top', `${height}px`);
      workspace.style.setProperty('--workspace-room', `${Math.max(0, view - height)}px`);
    };
    measure();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    observer?.observe(top);
    if (scroller) observer?.observe(scroller);
    window.addEventListener('resize', measure);
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure); };
  }, [top]);
  return setTop;
}
