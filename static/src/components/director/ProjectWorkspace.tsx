import { useCallback, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ChevronDown, ChevronRight, Link2, Unlink } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { ProjectPlanEditor } from '../ProjectSchedulerDialog';
import FramingView from './FramingView';
import PlanEditor from './PlanEditor';
import ObservingPreferences from './ObservingPreferences';
import ActivationPanel, { RigActivation } from './ActivationPanel';
import type { DirectorActivationReport, DirectorRigProfileSummary } from '../../api/directorTypes';
import type { FramingSeed } from './framingModel';
import { retryWhenBusy } from './retry';
import { withoutPlanningParams } from '../../hooks/useUrlState';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** One global project: framing, then one block per rig with its plan, its
 *  database project, what activation does there and its Target Scheduler
 *  rows; then activation and the project priority. */
export default function ProjectWorkspace({ instanceId, projectId }: { instanceId: string; projectId: string }) {
  const [params] = useSearchParams();
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
  const [openSource, setOpenSource] = useState<string | null | undefined>(undefined);
  const arrival = row?.links.find(link => link.catalog_slug === cameFrom && link.source_row_id !== null);
  const openKey = openSource === undefined
    ? arrival ? `${arrival.catalog_slug}:${arrival.source_project_guid}` : null
    : openSource;
  const back = withoutPlanningParams(params.toString());
  // Attaching: another plan's database project joins this plan; the other
  // plan is retired. Only databases this plan has no project in yet.
  const client = useQueryClient();
  const [attachPick, setAttachPick] = useState('');
  const [detachPick, setDetachPick] = useState<string | null>(null);
  const [notice, setNotice] = useState('');
  // The plan editor's unsaved edits, and the way to save them before an
  // activation preview reads the saved plan.
  const [unsavedPlan, setUnsavedPlan] = useState(false);
  const savePlanRef = useRef<(() => Promise<boolean>) | null>(null);
  const savePlan = useCallback(() => savePlanRef.current?.() ?? Promise.resolve(true), []);
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
  const [report, setReport] = useState<DirectorActivationReport | null>(null);
  const onReport = useCallback((next: DirectorActivationReport | null) => setReport(next), []);
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const profiledRigs = useMemo(() => new Set((profiles.data ?? []).map(profile => profile.rig.id)), [profiles.data]);
  if (plans.isPending) return <p role="status">Loading project...</p>;
  if (plans.isError) return <div role="alert"><p>{message(plans.error)}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>;
  if (!row) return <p role="alert">Project not found. <Link to={`/?${back}`}>Back to the Library</Link></p>;
  const linkFor = (rigId: string) => row.links.find(link => link.rig.id === rigId);
  // A database's project, its detach control and its Target Scheduler rows,
  // shown with the rig in the plan or, for a database no rig profile covers,
  // on their own below the rigs.
  const databaseOf = (link: (typeof row.links)[number]) => {
    const key = `${link.catalog_slug}:${link.source_project_guid}`;
    const open = openKey === key;
    return <div className="director-rig-database" key={key}>
      <div className="director-actions">
        {link.source_row_id !== null && <button type="button" aria-expanded={open} onClick={() => setOpenSource(open ? null : key)}>{open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Edit in Target Scheduler</button>}
        {canWrite && row.links.length > 1 && <button type="button" aria-label={`Detach ${link.catalog_name}`} title="Give this database's project a plan of its own" onClick={() => { setDetachPick(detachPick === key ? null : key); setProblem(''); }}><Unlink size={16} />Detach</button>}
      </div>
      {detachPick === key && <p className="director-muted" role="note">{link.source_name ?? 'This project'} in {link.catalog_name} becomes a plan of its own; this plan keeps its drafts.
        <span className="director-actions"><button type="button" disabled={detach.isPending} onClick={() => detach.mutate(link)}>{detach.isPending ? 'Detaching…' : 'Detach'}</button><button type="button" onClick={() => setDetachPick(null)}>Cancel</button></span></p>}
      {open && link.source_row_id !== null && <ProjectPlanEditor dbId={link.catalog_slug} projectId={link.source_row_id} canEdit={canWrite && !!info.data?.allow_database_management} />}
    </div>;
  };
  const reportFor = (rigId: string) => report?.rigs.find(entry => entry.rig.id === rigId);
  const rigExtras = (rig: DirectorRigProfileSummary) => {
    const link = linkFor(rig.rig.id);
    const activation = reportFor(rig.rig.id);
    return {
      place: link ? link.source_name ? `project “${link.source_name}”` : 'Project row missing in this database' : 'no project yet',
      below: (link || activation) && <>
        {activation && report && <RigActivation rig={activation} applied={report.applied} />}
        {link && databaseOf(link)}
      </>,
    };
  };
  const unprofiled = row.links.filter(link => !profiledRigs.has(link.rig.id));
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
    {row.links.length === 0 && <p className="director-muted">No database holds this project yet; activation creates it in each rig you tick, or attach a project a database already has.</p>}
    {!manageable && row.links.length > 0 && <p className="director-muted">Target Scheduler rows are view only on this server.</p>}
    {unprofiled.map(link => <div key={link.catalog_slug} className="plan-rig" role="group" aria-label={link.catalog_name}>
      <p className="plan-rig-head"><strong>{link.catalog_name}</strong><span className="plan-rig-place">{link.source_name ? `project “${link.source_name}”` : 'Project row missing in this database'}</span></p>
      {databaseOf(link)}
    </div>)}
  </>;
  return <section aria-label="Project planning" className="director-workspace">
    <div className="director-toolbar director-workspace-head"><Link to={`/?${back}`}><ArrowLeft size={16} />Library</Link><h2>{row.project.name}</h2></div>
    <h3 className="director-section-heading director-framing-heading">Framing</h3>
    {first && scheduler.isPending ? <p role="status">Loading targets...</p> : <FramingView projectId={projectId} seed={seed} preferredRigIds={row.links.map(link => link.rig.id)} />}
    <h3 className="director-section-heading">Plan</h3>
    <PlanEditor projectId={projectId} linkedRigIds={row.links.map(link => link.rig.id)} rigExtras={rigExtras} footer={attachArea} onUnsavedChange={setUnsavedPlan} saveRef={savePlanRef} />
    <h3 className="director-section-heading">Activation</h3>
    <ActivationPanel projectId={projectId} onReport={onReport} shownElsewhere={profiledRigs} unsavedPlan={unsavedPlan} savePlan={savePlan} />
    <ObservingPreferences projectId={projectId} rigs={row.links.map(link => ({ id: link.rig.id, name: link.catalog_name }))} projects={(plans.data?.rows ?? []).map(entry => entry.project)} />
  </section>;
}
