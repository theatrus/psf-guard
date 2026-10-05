import { type KeyboardEvent as ReactKeyboardEvent, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { ChevronDown, ChevronRight, Link2, Unlink } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { ProjectPlanEditor } from '../ProjectSchedulerDialog';
import FramingView from './FramingView';
import PlanEditor from './PlanEditor';
import PlanScheduling from './PlanScheduling';
import ObservingPreferences from './ObservingPreferences';
import ActivationPanel from './ActivationPanel';
import { DraftProvider, EditedMark, SaveBar } from './pageDrafts';
import WorkspaceSummary from './WorkspaceSummary';
import { usePageDrafts } from './pageDraftsState';
import type { DirectorRigProfileSummary } from '../../api/directorTypes';
import type { FramingSeed } from './framingModel';
import { retryWhenBusy } from './retry';
import { withoutPlanningParams } from '../../hooks/useUrlState';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** The workspace's tabs, one job each. Every panel stays mounted, hidden
 *  when not shown, so an unsaved edit survives a switch; `draft` names the
 *  save-bar section a tab holds. */
const TABS = [
  { id: 'framing', label: 'Framing', drafts: ['framing'] },
  { id: 'plan', label: 'Plan', drafts: ['plan', 'scheduling'] },
  { id: 'activate', label: 'Activate', drafts: [] },
  { id: 'databases', label: 'Rig databases', drafts: [] },
  { id: 'priority', label: 'Priority and defaults', drafts: ['priority'] },
] as const;
type TabId = (typeof TABS)[number]['id'];

/** One global project: a summary of what it is and where it stands, then
 *  one tab at a time for framing, the plan, activation, each rig database's
 *  Target Scheduler rows, and the project priority. */
export default function ProjectWorkspace({ instanceId, projectId }: { instanceId: string; projectId: string }) {
  const [params, setParams] = useSearchParams();
  const { canWrite } = useAccess();
  const info = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 300_000 });
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
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
  // Each database's Target Scheduler rows are open in their own tab; the one
  // the Library's link came from stays open first however others are folded.
  const [closed, setClosed] = useState<Set<string>>(() => new Set());
  const arrival = row?.links.find(link => link.catalog_slug === cameFrom && link.source_row_id !== null);
  const back = withoutPlanningParams(params.toString());
  // Attaching: another plan's database project joins this plan; the other
  // plan is retired. Only databases this plan has no project in yet.
  const client = useQueryClient();
  const [attachPick, setAttachPick] = useState('');
  const [detachPick, setDetachPick] = useState<string | null>(null);
  const [notice, setNotice] = useState('');
  // Framing, plan and priority edits wait in one draft for the save bar;
  // activation saves them before it reads the saved plan.
  const drafts = usePageDrafts();
  const [problem, setProblem] = useState('');
  const candidates = useMemo(() => {
    if (!plans.data || !row) return [];
    const taken = new Set(row.links.map(link => link.catalog_slug));
    return plans.data.rows.filter(other => other.project.id !== projectId && other.links.length > 0 && other.links.every(link => !taken.has(link.catalog_slug)))
      .flatMap(other => other.links.map(link => ({ key: `${other.project.id}:${link.catalog_slug}:${link.source_project_guid}`, plan: other, link })));
  }, [plans.data, row, projectId]);
  const chosen = candidates.find(entry => entry.key === attachPick) ?? null;
  const attach = useMutation({
    retry: false,
    mutationFn: (fromProjectId: string) => apiClient.attachDirectorProject(projectId, fromProjectId),
    onSuccess: done => {
      setAttachPick(''); setProblem('');
      setNotice(`Attached ${done.absorbed.name}: ${done.moved_links} database${done.moved_links === 1 ? '' : 's'} joined this plan${done.framing_taken ? ', and its framing came along' : ''}${done.plan_taken ? ', and its plan came along' : ''}.`);
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
      void client.invalidateQueries({ queryKey: ['directorFraming', projectId] });
      void client.invalidateQueries({ queryKey: ['directorPlan', projectId] });
    },
    onError: error => setProblem(message(error)),
  });
  const detach = useMutation({
    retry: false,
    mutationFn: (link: { catalog_slug: string; source_project_guid: string; source_name: string | null }) => apiClient.detachDirectorProject(projectId, link.catalog_slug, link.source_project_guid, link.source_name ?? row?.project.name ?? 'Project'),
    onSuccess: fresh => { setDetachPick(null); setProblem(''); setNotice(`Detached: ${fresh.name} is a plan of its own again.`); void client.invalidateQueries({ queryKey: ['directorPlans'] }); },
    onError: error => setProblem(message(error)),
  });
  // The tab lives in the address, so a reload or a shared link opens it.
  // Without one, a plan with no framing yet starts there; any other on its plan.
  const framingDraft = useQuery({ queryKey: ['directorFraming', projectId], queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const asked = params.get('planTab');
  const tab: TabId = TABS.some(entry => entry.id === asked) ? asked as TabId
    : framingDraft.data && !framingDraft.data.draft ? 'framing' : 'plan';
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
  const linkFor = (rigId: string) => row.links.find(link => link.rig.id === rigId);
  // A database's project, its detach control and its Target Scheduler rows,
  // shown with the rig in the plan or, for a database no rig profile covers,
  // on their own below the rigs.
  const databaseOf = (link: (typeof row.links)[number]) => {
    const key = `${link.catalog_slug}:${link.source_project_guid}`;
    const open = !closed.has(key);
    const toggle = () => setClosed(current => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next; });
    return <div className="director-rig-database plan-rig" role="group" aria-label={link.catalog_name} key={key}>
      <p className="plan-rig-head"><strong>{link.catalog_name}</strong><span className="plan-rig-place">{link.source_name ? `project “${link.source_name}”` : 'Project row missing in this database'}</span>{arrival === link && <span className="director-muted"> · opened from here</span>}</p>
      <div className="director-actions">
        {link.source_row_id !== null && <button type="button" aria-expanded={open} onClick={toggle}>{open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Target Scheduler rows</button>}
        {canWrite && row.links.length > 1 && <button type="button" aria-label={`Detach ${link.catalog_name}`} title="Give this database's project a plan of its own" onClick={() => { setDetachPick(detachPick === key ? null : key); setProblem(''); }}><Unlink size={16} />Detach</button>}
      </div>
      {detachPick === key && <p className="director-muted" role="note">{link.source_name ?? 'This project'} in {link.catalog_name} becomes a plan of its own; this plan keeps its drafts.
        <span className="director-actions"><button type="button" disabled={detach.isPending} onClick={() => detach.mutate(link)}>{detach.isPending ? 'Detaching…' : 'Detach'}</button><button type="button" onClick={() => setDetachPick(null)}>Cancel</button></span></p>}
      {open && visited.has('databases') && link.source_row_id !== null && <ProjectPlanEditor dbId={link.catalog_slug} projectId={link.source_row_id} canEdit={canWrite && !!info.data?.allow_database_management} />}
    </div>;
  };
  const rigExtras = (rig: DirectorRigProfileSummary) => {
    const link = linkFor(rig.rig.id);
    return { place: link ? link.source_name ? `project “${link.source_name}”` : 'Project row missing in this database' : 'no project yet' };
  };
  const attachArea = <>
    {canWrite && candidates.length > 0 && <div className="director-attach">
      <label>Attach a project from another database
        <select aria-label="Attach a project from another database" value={attachPick} onChange={event => { setAttachPick(event.target.value); setProblem(''); }}>
          <option value="">Choose a project…</option>
          {candidates.map(entry => <option key={entry.key} value={entry.key}>{entry.link.catalog_name}: {entry.link.source_name ?? entry.link.source_project_guid}{entry.plan.project.name !== (entry.link.source_name ?? '') ? ` (plan “${entry.plan.project.name}”)` : ''}</option>)}
        </select></label>
      {chosen && <p className="director-muted" role="note">
        {chosen.plan.links.length > 1 ? `The plan “${chosen.plan.project.name}” and its ${chosen.plan.links.length} databases join this plan` : `“${chosen.plan.project.name}” joins this plan and is retired`}; the next activation takes its targets over.
        <span className="director-actions"><button type="button" disabled={attach.isPending} onClick={() => attach.mutate(chosen.plan.project.id)}><Link2 size={16} />{attach.isPending ? 'Attaching…' : 'Attach'}</button><button type="button" onClick={() => setAttachPick('')}>Cancel</button></span>
      </p>}
    </div>}
    {notice && <p role="status">{notice}</p>}
    {problem && <p className="director-error" role="alert">{problem}</p>}
  </>;
  // The arrival database first, then the rest in the plan's order.
  const databases = arrival ? [arrival, ...row.links.filter(link => link !== arrival)] : row.links;
  const drafted = (ids: readonly string[]) => drafts.unsaved.some(section => ids.includes(section.id));
  const summaryRigs = [...new Map(row.links.map(link => [link.rig.id, { id: link.rig.id, name: link.catalog_name }])).values()];
  const panel = (id: TabId) => ({ role: 'tabpanel' as const, id: `workspace-panel-${id}`, 'aria-labelledby': `workspace-tab-${id}`, hidden: tab !== id, className: 'workspace-panel' });
  return <DraftProvider drafts={drafts}><section aria-label="Project planning" className="director-workspace">
    <div className="workspace-top">
      <SaveBar drafts={drafts} canWrite={canWrite} />
      <WorkspaceSummary projectId={projectId} projectName={row.project.name} back={back.toString()} rigs={summaryRigs} />
      <div className="workspace-tabs" role="tablist" aria-label="Plan sections" onKeyDown={onTabKey}>
        {TABS.map(entry => <button key={entry.id} type="button" role="tab" id={`workspace-tab-${entry.id}`} aria-selected={entry.id === tab} aria-controls={`workspace-panel-${entry.id}`}
          tabIndex={entry.id === tab ? 0 : -1} className={`workspace-tab${entry.id === tab ? ' active' : ''}`} onClick={() => chooseTab(entry.id)}>
          {entry.label}{drafted(entry.drafts) && <span className="workspace-tab-edited" aria-label="edited" title="Unsaved changes">●</span>}
        </button>)}
      </div>
    </div>
    <div {...panel('framing')}>
      {first && scheduler.isPending ? <p role="status">Loading targets...</p> : <FramingView projectId={projectId} seed={seed} preferredRigIds={row.links.map(link => link.rig.id)} />}
    </div>
    <div {...panel('plan')}>
      <PlanEditor projectId={projectId} linkedRigIds={row.links.map(link => link.rig.id)} rigExtras={rigExtras} footer={attachArea} />
      <h3 className="director-section-heading">Scheduling limits<EditedMark drafts={drafts} id="scheduling" /></h3>
      <PlanScheduling projectId={projectId} rigs={summaryRigs} />
    </div>
    <div {...panel('activate')}>
      <ActivationPanel projectId={projectId} />
    </div>
    <div {...panel('databases')}>
      <p className="director-muted">Each database's own Target Scheduler rows for this project. Activation writes the plan's targets, exposure plans and scheduling limits here; everything else is Target Scheduler's.</p>
      {row.links.length === 0 && <p className="director-muted">No database holds this project yet; activation creates it in each rig you tick, or attach a project a database already has.</p>}
      {!manageable && row.links.length > 0 && <p className="director-muted">Target Scheduler rows are view only on this server.</p>}
      {databases.map(databaseOf)}
    </div>
    <div {...panel('priority')}>
      <ObservingPreferences projectId={projectId} folded={false} rigs={row.links.map(link => ({ id: link.rig.id, name: link.catalog_name }))} projects={(plans.data?.rows ?? []).map(entry => entry.project)} />
    </div>
  </section></DraftProvider>;
}
