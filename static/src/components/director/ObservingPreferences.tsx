import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, RefreshCw, Save } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { ObservingSettings, PreferenceScope } from '../../api/directorPreferences';
import './ObservingPreferences.css';

type Project = { id: string; name: string };
const errorText = (error: unknown) => error instanceof Error ? error.message : 'Project priority failed';
function ordered(projects: Project[], ids: string[]) {
  const ranks = new Map(ids.map((id, i) => [id, i]));
  const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
  return [...projects].sort((a, b) => (ranks.get(a.id) ?? Infinity) - (ranks.get(b.id) ?? Infinity) || compare(a.name, b.name) || compare(a.id, b.id));
}

export default function ObservingPreferences({ projectId, rigs, projects }: { projectId: string; rigs: Project[]; projects: Project[] }) {
  const [rigPick, setRigPick] = useState('');
  const [scope, setScope] = useState<PreferenceScope>('global');
  const available = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles });
  const choices = [...rigs, ...(available.data ?? []).filter(r => !rigs.some(existing => existing.id === r.rig.id)).map(r => ({ id: r.rig.id, name: r.catalog_name }))];
  const rig = choices.some(r => r.id === rigPick) ? rigPick : choices[0]?.id ?? '';
  const defaults = useQuery({ queryKey: ['observingDefaults'], queryFn: apiClient.getObservingDefaults });
  const effective = useQuery({ queryKey: ['observingEffective', rig], queryFn: () => apiClient.getEffectiveObserving(rig), enabled: !!rig, refetchOnWindowFocus: false });
  const site = effective.data?.settings.find(s => s.scope === 'rig')?.site_id;
  const id = scope === 'rig' ? rig : scope === 'site' ? site : defaults.data?.global_id;
  const settings = useQuery({ queryKey: ['observingSettings', scope, id], queryFn: () => apiClient.getObservingSettings(scope, id!), enabled: !!id, refetchOnWindowFocus: false });
  const globalOrder = effective.data?.settings.find(s => s.scope === 'global')?.project_order ?? [];
  return <section className="observing-preferences" aria-label="Project priority">
    <h3>Project priority</h3>
    <div className="observing-context">
      <label>Scope<select aria-label="Priority scope" value={scope} onChange={e => setScope(e.target.value as PreferenceScope)}><option value="global">Global order</option><option value="site" disabled={!site}>Site override</option><option value="rig" disabled={!rig}>Rig override</option></select></label>
      <label>Rig<select aria-label="Priority rig" value={rig} onChange={e => setRigPick(e.target.value)}>{choices.map(r => <option key={r.id} value={r.id}>{r.name}</option>)}</select></label>
      {effective.data && <span className="observing-mode">{effective.data.order_source ? `Following ${effective.data.order_source.scope} order` : 'Previous scheduling policy active'}</span>}
    </div>
    {(settings.error || effective.error || defaults.error || available.error) && <p role="alert">{errorText(settings.error ?? effective.error ?? defaults.error ?? available.error)}</p>}
    {id && settings.isPending && <p role="status">Loading priority...</p>}
    {settings.data && defaults.data && (scope === 'global' || effective.data) && <PriorityEditor key={`${scope}:${id}`} initial={settings.data} globalOrder={globalOrder} projects={projects} currentProject={projectId} sites={defaults.data.sites} />}
  </section>;
}

function PriorityEditor({ initial, globalOrder, projects, currentProject, sites }: { initial: ObservingSettings; globalOrder: string[]; projects: Project[]; currentProject: string; sites: Project[] }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [draft, setDraft] = useState(initial);
  const [saved, setSaved] = useState(false);
  const inherit = initial.scope !== 'global' && draft.project_order == null;
  const site = useQuery({ queryKey: ['observingSettings', 'site', draft.site_id], queryFn: () => apiClient.getObservingSettings('site', draft.site_id!), enabled: initial.scope === 'rig' && !!draft.site_id, refetchOnWindowFocus: false });
  const needsSite = inherit && initial.scope === 'rig' && !!draft.site_id;
  const parentPending = needsSite && (site.isPending || !!site.error);
  const inherited = initial.scope === 'rig' && draft.site_id ? site.data?.project_order ?? globalOrder : globalOrder;
  const list = ordered(projects, draft.project_order ?? inherited);
  const reload = useMutation({ mutationFn: () => apiClient.getObservingSettings(initial.scope, initial.scope_id), retry: false, onSuccess: fresh => { setDraft(fresh); setSaved(false); save.reset(); } });
  const save = useMutation({ mutationFn: () => apiClient.saveObservingSettings({ ...draft, project_order: inherit ? null : list.map(p => p.id) }), retry: false, onSuccess: result => {
    setSaved(true); setDraft(result);
    client.setQueryData(['observingSettings', result.scope, result.scope_id], result);
    void client.invalidateQueries({ queryKey: ['observingEffective'] });
  } });
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
      <button type="submit" disabled={list.length === 0 || list.length > 256 || parentPending}><Save size={16} />{save.isPending ? 'Saving...' : 'Save priority'}</button>
      {needsSite && site.isPending && <p role="status">Loading site priority...</p>}
      {needsSite && site.error && <p role="alert">{errorText(site.error)} <button type="button" onClick={() => void site.refetch()}><RefreshCw size={16} />Retry</button></p>}
      {list.length > 256 && <p role="alert">A priority order supports up to 256 projects.</p>}
    </fieldset>
    {save.error && <p role="alert">{errorText(reload.error ?? save.error)} <button type="button" disabled={reload.isPending} onClick={() => reload.mutate()}><RefreshCw size={16} />Reload saved priority</button></p>}
    {saved && <p role="status">Project priority saved.</p>}
  </form>;
}
