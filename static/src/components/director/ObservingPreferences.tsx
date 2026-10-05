import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, RefreshCw, Save } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { EditedMark } from './pageDrafts';
import { useDraftSection, useDrafts } from './pageDraftsState';
import type { ObservingSettings, PreferenceScope, SchedulingOverrides } from '../../api/directorPreferences';
import SchedulingFields from './SchedulingFields';
import { compactOverrides, inheritedLimits } from './schedulingModel';
import './ObservingPreferences.css';

type Project = { id: string; name: string };
const errorText = (error: unknown) => error instanceof Error ? error.message : 'Project priority failed';
function ordered(projects: Project[], ids: string[]) {
  const ranks = new Map(ids.map((id, i) => [id, i]));
  const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
  return [...projects].sort((a, b) => (ranks.get(a.id) ?? Infinity) - (ranks.get(b.id) ?? Infinity) || compare(a.name, b.name) || compare(a.id, b.id));
}

export default function ObservingPreferences({ projectId, rigs, projects, folded = true }: { projectId: string; rigs: Project[]; projects: Project[]; folded?: boolean }) {
  const [rigPick, setRigPick] = useState('');
  const [scope, setScope] = useState<PreferenceScope>('global');
  // Switching scope or rig shows another order and would drop an edit to
  // this one, so both wait until it is saved or discarded.
  const [editing, setEditing] = useState(false);
  const drafts = useDrafts();
  const available = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles });
  const choices = [...rigs, ...(available.data ?? []).filter(r => !rigs.some(existing => existing.id === r.rig.id)).map(r => ({ id: r.rig.id, name: r.catalog_name }))];
  const rig = choices.some(r => r.id === rigPick) ? rigPick : choices[0]?.id ?? '';
  const defaults = useQuery({ queryKey: ['observingDefaults'], queryFn: apiClient.getObservingDefaults });
  const effective = useQuery({ queryKey: ['observingEffective', rig], queryFn: () => apiClient.getEffectiveObserving(rig), enabled: !!rig, refetchOnWindowFocus: false });
  const site = effective.data?.settings.find(s => s.scope === 'rig')?.site_id;
  const id = scope === 'rig' ? rig : scope === 'site' ? site : defaults.data?.global_id;
  const settings = useQuery({ queryKey: ['observingSettings', scope, id], queryFn: () => apiClient.getObservingSettings(scope, id!), enabled: !!id, refetchOnWindowFocus: false });
  const globalOrder = effective.data?.settings.find(s => s.scope === 'global')?.project_order ?? [];
  // Folded, the ranking is about every plan; the line says where this one stands.
  const place = ordered(projects, globalOrder).findIndex(project => project.id === projectId);
  const body = <>
    <div className="observing-context">
      <label>Scope<select aria-label="Priority scope" value={scope} disabled={editing} title={editing ? 'Save or discard the priority change first' : undefined} onChange={e => setScope(e.target.value as PreferenceScope)}><option value="global">Global order</option><option value="site" disabled={!site}>Site override</option><option value="rig" disabled={!rig}>Rig override</option></select></label>
      <label>Rig<select aria-label="Priority rig" value={rig} disabled={editing} title={editing ? 'Save or discard the priority change first' : undefined} onChange={e => setRigPick(e.target.value)}>{choices.map(r => <option key={r.id} value={r.id}>{r.name}</option>)}</select></label>
      {effective.data && <span className="observing-mode">{effective.data.order_source ? `Following ${effective.data.order_source.scope} order` : 'Previous scheduling policy active'}</span>}
    </div>
    {(settings.error || effective.error || defaults.error || available.error) && <p role="alert">{errorText(settings.error ?? effective.error ?? defaults.error ?? available.error)}</p>}
    {id && settings.isPending && <p role="status">Loading priority...</p>}
    {settings.data && defaults.data && (scope === 'global' || effective.data) && <PriorityEditor key={`${scope}:${id}`} initial={settings.data} globalOrder={globalOrder} globalScheduling={effective.data?.settings.find(s => s.scope === 'global')?.scheduling} projects={projects} currentProject={projectId} sites={defaults.data.sites} onEditing={setEditing} />}
  </>;
  return <section className="observing-preferences" aria-label="Project priority">
    {folded
      ? <details className="observing-fold">
    <summary><h3>Project priority{drafts && <EditedMark drafts={drafts} id="priority" />}</h3>{effective.data && place >= 0 && <span className="director-muted"> · this plan is {place + 1} of {projects.length} in the global order</span>}</summary>
    {body}
    </details>
      : <><div className="observing-heading"><h3>Project priority{drafts && <EditedMark drafts={drafts} id="priority" />}</h3>{effective.data && place >= 0 && <span className="director-muted"> · this plan is {place + 1} of {projects.length} in the global order</span>}</div>{body}</>}
  </section>;
}

function PriorityEditor({ initial, globalOrder, globalScheduling, projects, currentProject, sites, onEditing }: { initial: ObservingSettings; globalOrder: string[]; globalScheduling?: SchedulingOverrides; projects: Project[]; currentProject: string; sites: Project[]; onEditing?: (editing: boolean) => void }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [draft, setDraft] = useState(initial);
  const [stored, setStored] = useState(initial);
  const [saved, setSaved] = useState(false);
  const inherit = initial.scope !== 'global' && draft.project_order == null;
  const site = useQuery({ queryKey: ['observingSettings', 'site', draft.site_id], queryFn: () => apiClient.getObservingSettings('site', draft.site_id!), enabled: initial.scope === 'rig' && !!draft.site_id, refetchOnWindowFocus: false });
  const needsSite = inherit && initial.scope === 'rig' && !!draft.site_id;
  const parentPending = needsSite && (site.isPending || !!site.error);
  const inherited = initial.scope === 'rig' && draft.site_id ? site.data?.project_order ?? globalOrder : globalOrder;
  const list = ordered(projects, draft.project_order ?? inherited);
  // The scheduling defaults this scope inherits: Target Scheduler's, then
  // every plan's, then the rig's site.
  const parentLimits = inheritedLimits([
    initial.scope !== 'global' ? { overrides: globalScheduling, from: 'from every plan' } : null,
    initial.scope === 'rig' && draft.site_id ? { overrides: site.data?.scheduling, from: 'from the site' } : null,
  ]);
  const reload = useMutation({ mutationFn: () => apiClient.getObservingSettings(initial.scope, initial.scope_id), retry: false, onSuccess: fresh => { setDraft(fresh); setStored(fresh); setSaved(false); save.reset(); } });
  const save = useMutation({ mutationFn: () => apiClient.saveObservingSettings({ ...draft, project_order: inherit ? null : list.map(p => p.id) }), retry: false, onSuccess: result => {
    setSaved(true); setDraft(result); setStored(result);
    client.setQueryData(['observingSettings', result.scope, result.scope_id], result);
    void client.invalidateQueries({ queryKey: ['observingEffective'] });
  } });
  const unsaved = canWrite && JSON.stringify(draft) !== JSON.stringify(stored);
  useEffect(() => { onEditing?.(unsaved); }, [unsaved, onEditing]);
  useEffect(() => () => onEditing?.(false), [onEditing]);
  const savable = !parentPending && list.length > 0 && list.length <= 256;
  const managed = useDraftSection('priority', {
    label: 'Priority and defaults',
    order: 3,
    unsaved,
    changes: [
      JSON.stringify(draft.project_order ?? null) !== JSON.stringify(stored.project_order ?? null) && (draft.project_order == null ? 'back to the inherited order' : 'project order'),
      draft.site_id !== stored.site_id && 'planning site',
    ].filter((change): change is string => !!change),
    save: async () => {
      if (!savable) return false;
      try { await save.mutateAsync(); } catch (error) { return error instanceof Error ? error.message : false; }
      return true;
    },
    discard: () => { setDraft(stored); setSaved(false); save.reset(); },
  });
  const change = (patch: Partial<ObservingSettings>) => { setSaved(false); setDraft(current => ({ ...current, ...patch })); };
  const move = (index: number, delta: number) => {
    const ids = list.map(p => p.id);
    [ids[index], ids[index + delta]] = [ids[index + delta], ids[index]];
    change({ project_order: ids });
  };
  return <form onSubmit={e => { e.preventDefault(); if (canWrite && !save.isPending && !parentPending && list.length > 0 && list.length <= 256) save.mutate(); }}>
    <fieldset disabled={!canWrite || save.isPending || reload.isPending}>
      <legend>{initial.scope === 'global' ? 'Global order' : `${initial.scope === 'rig' ? 'Rig' : 'Site'} override`}</legend>
      {initial.scope !== 'global' && <label className="priority-inherit"><input type="checkbox" checked={inherit} disabled={parentPending} onChange={e => change({ project_order: e.target.checked ? null : list.map(p => p.id) })} />Use inherited order</label>}
      {initial.scope === 'rig' && <label>Planning site<select aria-label="Planning site" value={draft.site_id ?? ''} onChange={e => change({ site_id: e.target.value || null })}><option value="">None</option>{sites.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}</select></label>}
      <ol className="project-priority-list" aria-label="Ranked projects">
        {list.map((project, index) => <li key={project.id} aria-current={project.id === currentProject ? 'true' : undefined}>
          <span className="priority-position">{index + 1}</span><span className="priority-project-name">{project.name}</span>
          <button type="button" title={`Move ${project.name} up`} aria-label={`Move ${project.name} up`} disabled={inherit || index === 0} onClick={() => move(index, -1)}><ArrowUp size={16} /></button>
          <button type="button" title={`Move ${project.name} down`} aria-label={`Move ${project.name} down`} disabled={inherit || index === list.length - 1} onClick={() => move(index, 1)}><ArrowDown size={16} /></button>
        </li>)}
      </ol>
      {list.length === 0 && <p>No projects.</p>}
      {!managed && <button type="submit" disabled={list.length === 0 || list.length > 256 || parentPending}><Save size={16} />{save.isPending ? 'Saving...' : 'Save priority'}</button>}
      {needsSite && site.isPending && <p role="status">Loading site priority...</p>}
      {needsSite && site.error && <p role="alert">{errorText(site.error)} <button type="button" onClick={() => void site.refetch()}><RefreshCw size={16} />Retry</button></p>}
      {list.length > 256 && <p role="alert">A priority order supports up to 256 projects.</p>}
    </fieldset>
    <fieldset disabled={!canWrite || save.isPending || reload.isPending} className="scheduling-defaults">
      <legend>Scheduling defaults{initial.scope === 'global' ? ' for every plan' : initial.scope === 'site' ? ' for this site' : ' for this rig'}</legend>
      <p className="director-muted">Target Scheduler limits each plan starts from{initial.scope === 'global' ? '' : '; empty fields follow the scope above'}. A plan can set its own on its Plan tab, and activation writes the result into each rig's Target Scheduler project.</p>
      <SchedulingFields label={`${initial.scope} scheduling defaults`} overrides={draft.scheduling ?? {}} onChange={next => change({ scheduling: compactOverrides(next) })}
        inherited={parentLimits.values} inheritedFrom={limit => parentLimits.from[limit]} disabled={!canWrite} />
    </fieldset>
    {save.error && <p role="alert">{errorText(reload.error ?? save.error)} <button type="button" disabled={reload.isPending} onClick={() => reload.mutate()}><RefreshCw size={16} />Reload saved priority</button></p>}
    {saved && <p role="status">Project priority saved.</p>}
  </form>;
}
