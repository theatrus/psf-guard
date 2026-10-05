import { type MutableRefObject, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, Link2, Minus, Plus, Unlink } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { ProjectPlanEditor } from '../ProjectSchedulerDialog';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import type { PlanRigControls } from './PlanEditor';
import { useActivationState } from './activationState';
import { retryWhenBusy } from './retry';
import './PlanRigs.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** The rigs that shoot a plan and what each holds in Target Scheduler: add
 *  a rig with a new project or one its database already has, drop or
 *  detach one, and once a rig's database has the project, its Target
 *  Scheduler settings. */
export default function PlanRigs({ row, rows, joined, objectives, controls, cameFrom, load }: {
  row: DirectorPlanRow;
  /** Every plan, for projects other plans hold that could join this one. */
  rows: DirectorPlanRow[];
  /** Rigs shooting the plan, saved or not. */
  joined: string[];
  /** How many objectives the plan has, saved or not: a rig joins by shooting them. */
  objectives: number;
  controls: MutableRefObject<PlanRigControls | null>;
  /** The database the Library's link came from; it is listed first. */
  cameFrom: string | null;
  /** Load each database's Target Scheduler rows; false until the tab is opened. */
  load: boolean;
}) {
  const projectId = row.project.id;
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const info = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 300_000 });
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const { last } = useActivationState(projectId);
  const client = useQueryClient();
  const [pick, setPick] = useState('');
  const [detachPick, setDetachPick] = useState<string | null>(null);
  // Each project's settings are long; the database the Library's link came
  // from opens, or the only one, and the rest wait for a click.
  const [opened, setOpened] = useState<Set<string>>(() => {
    const lead = row.links.find(link => link.catalog_slug === cameFrom) ?? (row.links.length === 1 ? row.links[0] : undefined);
    return new Set(lead ? [`${lead.catalog_slug}:${lead.source_project_guid}`] : []);
  });
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');

  // Projects other plans hold in databases this plan has none in.
  const candidates = useMemo(() => {
    const taken = new Set(row.links.map(link => link.catalog_slug));
    return rows.filter(other => other.project.id !== projectId && other.links.length > 0 && other.links.every(link => !taken.has(link.catalog_slug)))
      .flatMap(other => other.links.map(link => ({ key: `attach:${other.project.id}:${link.catalog_slug}:${link.source_project_guid}`, plan: other, link })));
  }, [rows, row, projectId]);
  const linkedRigs = new Set(row.links.map(link => link.rig.id));
  const fresh = (profiles.data ?? []).filter(rig => !linkedRigs.has(rig.rig.id) && !joined.includes(rig.rig.id));
  const chosen = candidates.find(entry => entry.key === pick) ?? null;
  const attach = useMutation({
    retry: false,
    mutationFn: (fromProjectId: string) => apiClient.attachDirectorProject(projectId, fromProjectId),
    onSuccess: done => {
      setPick(''); setProblem('');
      setNotice(`Attached ${done.absorbed.name}: ${done.moved_links} database${done.moved_links === 1 ? '' : 's'} joined this plan${done.framing_taken ? ', and its framing came along' : ''}${done.plan_taken ? ', and its plan came along' : ''}.`);
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
      void client.invalidateQueries({ queryKey: ['directorFraming', projectId] });
      void client.invalidateQueries({ queryKey: ['directorPlan', projectId] });
    },
    onError: error => setProblem(message(error)),
  });
  const detach = useMutation({
    retry: false,
    mutationFn: (link: DirectorPlanLink) => apiClient.detachDirectorProject(projectId, link.catalog_slug, link.source_project_guid, link.source_name ?? row.project.name),
    onSuccess: plan => { setDetachPick(null); setProblem(''); setNotice(`Detached: ${plan.name} is a plan of its own again.`); void client.invalidateQueries({ queryKey: ['directorPlans'] }); },
    onError: error => setProblem(message(error)),
  });
  const addRig = (value: string) => {
    setProblem('');
    if (!value.startsWith('new:')) { setPick(value); setNotice(''); return; }
    const id = value.slice(4);
    controls.current?.setRig(id, true);
    setPick('');
    const name = profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'The rig';
    setNotice(`${name} joins the plan with a template for each objective; check them on Exposures. Activation creates the project in its database.`);
  };

  // One entry per rig: a database project linked to the plan, or a rig
  // shooting it that has no project yet. The Library's database leads.
  const entries = [
    ...row.links.map(link => ({ rigId: link.rig.id, name: link.catalog_name, link })),
    ...joined.filter(id => !linkedRigs.has(id)).map(id => ({ rigId: id, name: profiles.data?.find(rig => rig.rig.id === id)?.catalog_name ?? 'A rig', link: null })),
  ].sort((left, right) => Number(right.link?.catalog_slug === cameFrom) - Number(left.link?.catalog_slug === cameFrom));
  const activated = new Set(last.data?.rigs.map(rig => rig.rig_id) ?? []);
  const canEditRows = canWrite && !!info.data?.allow_database_management;

  return <section aria-label="Rigs" className="plan-rig-list">
    <p className="director-muted">Each rig shoots the plan from a project in its own Target Scheduler database. Activation writes the plan's targets, exposure plans and scheduling limits there; the rest of each project's settings are Target Scheduler's, edited below.</p>
    {entries.length === 0 && <p className="director-muted">No rig shoots this plan yet.</p>}
    {!manageable && row.links.length > 0 && <p className="director-muted">Target Scheduler rows are view only on this server.</p>}
    {entries.map(({ rigId, name, link }) => {
      const key = link ? `${link.catalog_slug}:${link.source_project_guid}` : `new:${rigId}`;
      const shooting = joined.includes(rigId);
      const open = opened.has(key);
      const toggle = () => setOpened(current => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next; });
      const place = link ? link.source_name ? `project “${link.source_name}”` : 'project row missing in this database' : 'new project on activation';
      return <div className="director-rig-database plan-rig" role="group" aria-label={name} key={key}>
        <p className="plan-rig-head"><strong>{name}</strong><span className="plan-rig-place">{place}</span>
          <span className="director-muted">{activated.has(rigId) ? `activated, revision ${last.data?.revision}` : 'not activated yet'}{shooting ? '' : ' · not shooting this plan'}</span>
          {link && cameFrom === link.catalog_slug && <span className="director-muted"> · opened from here</span>}</p>
        <div className="director-actions">
          {canWrite && !shooting && <button type="button" disabled={objectives === 0} title={objectives === 0 ? 'Add an objective on Exposures first' : undefined} onClick={() => controls.current?.setRig(rigId, true)}><Plus size={16} />Shoot this plan</button>}
          {canWrite && shooting && <button type="button" aria-label={`Drop ${name} from the plan`} title={link ? 'Stop shooting this plan here; the project stays in its database' : 'Leave this rig out of the plan'} onClick={() => controls.current?.setRig(rigId, false)}><Minus size={16} />Drop from plan</button>}
          {canWrite && link && row.links.length > 1 && <button type="button" aria-label={`Detach ${name}`} title="Give this database's project a plan of its own" onClick={() => { setDetachPick(detachPick === key ? null : key); setProblem(''); }}><Unlink size={16} />Detach</button>}
          {link && link.source_row_id !== null && <button type="button" aria-expanded={open} onClick={toggle}>{open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Target Scheduler settings</button>}
        </div>
        {link && detachPick === key && <p className="director-muted" role="note">{link.source_name ?? 'This project'} in {link.catalog_name} becomes a plan of its own; this plan keeps its drafts.
          <span className="director-actions"><button type="button" disabled={detach.isPending} onClick={() => detach.mutate(link)}>{detach.isPending ? 'Detaching…' : 'Detach'}</button><button type="button" onClick={() => setDetachPick(null)}>Cancel</button></span></p>}
        {link && link.source_row_id !== null
          ? open && load && <ProjectPlanEditor dbId={link.catalog_slug} projectId={link.source_row_id} canEdit={canEditRows} withTemplates={false} />
          : <p className="director-muted">Activate to create this project in {name}'s database; its Target Scheduler settings show here then.</p>}
      </div>;
    })}
    {canWrite && (fresh.length > 0 || candidates.length > 0) && <div className="director-attach">
      <label>Add a rig
        <select aria-label="Add a rig" value={pick} onChange={event => addRig(event.target.value)}>
          <option value="">Choose a rig…</option>
          {fresh.length > 0 && <optgroup label="With a new project">
            {fresh.map(rig => <option key={rig.rig.id} value={`new:${rig.rig.id}`} disabled={objectives === 0}>{rig.catalog_name}</option>)}
          </optgroup>}
          {candidates.length > 0 && <optgroup label="With a project its database has">
            {candidates.map(entry => <option key={entry.key} value={entry.key}>{entry.link.catalog_name}: {entry.link.source_name ?? entry.link.source_project_guid}{entry.plan.project.name !== (entry.link.source_name ?? '') ? ` (plan “${entry.plan.project.name}”)` : ''}</option>)}
          </optgroup>}
        </select></label>
      {objectives === 0 && fresh.length > 0 && <p className="director-muted">A rig joins with a new project by shooting the objectives; add one on Exposures first.</p>}
      {chosen && <p className="director-muted" role="note">
        {chosen.plan.links.length > 1 ? `The plan “${chosen.plan.project.name}” and its ${chosen.plan.links.length} databases join this plan` : `“${chosen.plan.project.name}” joins this plan and is retired`}; the next activation takes its targets over.
        <span className="director-actions"><button type="button" disabled={attach.isPending} onClick={() => attach.mutate(chosen.plan.project.id)}><Link2 size={16} />{attach.isPending ? 'Attaching…' : 'Attach'}</button><button type="button" onClick={() => setPick('')}>Cancel</button></span>
      </p>}
    </div>}
    {notice && <p role="status">{notice}</p>}
    {problem && <p className="director-error" role="alert">{problem}</p>}
  </section>;
}
