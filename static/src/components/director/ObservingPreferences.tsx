import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { RefreshCw, RotateCcw, Save } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { EffectiveObserving, ObservingSettings, PreferenceFactor, PreferenceScope } from '../../api/directorPreferences';
import './ObservingPreferences.css';

const factors: [PreferenceFactor, string][] = [['importance', 'Importance'], ['window_urgency', 'Closing window'], ['altitude', 'Altitude'], ['moon_opportunity', 'Moon-sensitive filters'], ['completion', 'Accepted progress'], ['efficiency', 'Efficiency'], ['continuity', 'Same target']];
const errorText = (error: unknown) => error instanceof Error ? error.message : 'Observing preferences failed';
const emptyOverrides = () => ({ weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null });

export default function ObservingPreferences({ projectId, rigs }: { projectId: string; rigs: { id: string; name: string }[] }) {
  const [rigPick, setRigPick] = useState('');
  const [scope, setScope] = useState<PreferenceScope>('project');
  const available = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles });
  const choices = [...rigs, ...(available.data ?? []).filter(r => !rigs.some(existing => existing.id === r.rig.id)).map(r => ({ id: r.rig.id, name: r.catalog_name }))];
  const rig = choices.some(r => r.id === rigPick) ? rigPick : choices[0]?.id ?? '';
  const defaults = useQuery({ queryKey: ['observingDefaults'], queryFn: apiClient.getObservingDefaults });
  const effective = useQuery({ queryKey: ['observingEffective', rig, projectId], queryFn: () => apiClient.getEffectiveObserving(rig, projectId), enabled: !!rig, refetchOnWindowFocus: false });
  const site = effective.data?.settings.find(s => s.scope === 'rig')?.site_id;
  const id = scope === 'project' ? projectId : scope === 'rig' ? rig : scope === 'site' ? site : defaults.data?.global_id;
  const settings = useQuery({ queryKey: ['observingSettings', scope, id], queryFn: () => apiClient.getObservingSettings(scope, id!), enabled: !!id, refetchOnWindowFocus: false });
  return <section className="observing-preferences" aria-label="Observing preferences">
    <h3>Observing preferences</h3>
    <div className="observing-context">
      <label>Rig<select aria-label="Preference rig" value={rig} onChange={e => setRigPick(e.target.value)}>{choices.map(r => <option key={r.id} value={r.id}>{r.name}</option>)}</select></label>
      <label>Scope<select aria-label="Preference scope" value={scope} onChange={e => setScope(e.target.value as PreferenceScope)}><option value="global">Global defaults</option><option value="site" disabled={!site}>Site</option><option value="rig">Rig</option><option value="project">Project</option></select></label>
      {effective.data && <span className="observing-mode">{effective.data.enabled ? 'Observing preferences enabled' : 'Legacy priority active'}</span>}
    </div>
    {!rig && <p>No rig selected for this project.</p>}
    {(settings.error || effective.error || defaults.error || available.error) && <p role="alert">{errorText(settings.error ?? effective.error ?? defaults.error ?? available.error)}</p>}
    {id && settings.isPending && <p role="status">Loading preferences...</p>}
    {settings.data && effective.data && defaults.data && <PreferenceEditor key={`${scope}:${id}`} initial={settings.data} effective={effective.data} defaults={defaults.data} />}
  </section>;
}

function PreferenceEditor({ initial, effective, defaults }: { initial: ObservingSettings; effective: EffectiveObserving; defaults: Awaited<ReturnType<typeof apiClient.getObservingDefaults>> }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [draft, setDraft] = useState(initial);
  const [preset, setPreset] = useState('');
  const [saved, setSaved] = useState(false);
  const reload = useMutation({ mutationFn: () => apiClient.getObservingSettings(initial.scope, initial.scope_id), retry: false, onSuccess: fresh => { setDraft(fresh); setPreset(''); setSaved(false); save.reset(); } });
  const save = useMutation({ mutationFn: () => apiClient.saveObservingSettings(draft), retry: false, onSuccess: result => {
    setSaved(true); setDraft(result);
    client.setQueryData(['observingSettings', result.scope, result.scope_id], result);
    void client.invalidateQueries({ queryKey: ['observingEffective'] });
  } });
  const change = (patch: Partial<ObservingSettings>) => { setSaved(false); setDraft(current => ({ ...current, ...patch })); };
  // Display the inherited value at this scope, not a downstream project's override.
  const order: PreferenceScope[] = ['global', 'site', 'rig', 'project'];
  const policy = { ...defaults.presets.balanced, weights: { ...defaults.presets.balanced.weights } };
  const sources: Record<string, string> = {};
  for (const settings of effective.settings.filter(s => order.indexOf(s.scope) < order.indexOf(initial.scope))) {
    for (const [key, value] of Object.entries(settings.overrides.weights)) { policy.weights[key as PreferenceFactor] = value!; sources[key] = settings.scope; }
    for (const key of ['importance', 'minimum_dwell_ms', 'switch_margin'] as const) if (settings.overrides[key] !== null) { policy[key] = settings.overrides[key]; sources[key] = settings.scope; }
  }
  const number = (key: 'importance' | 'minimum_dwell_ms' | 'switch_margin', label: string, max: number, scale = 1) => {
    const value = draft.overrides[key];
    return <div className="observing-row" key={key}>
      <label htmlFor={`observing-${key}`}>{label}<small>{value === null ? sources[key] ?? 'default' : initial.scope}</small></label>
      <input id={`observing-${key}`} type="number" min={0} max={max} step={1} value={(value ?? policy[key]) / scale} disabled={value === null} onChange={e => change({ overrides: { ...draft.overrides, [key]: Number(e.target.value) * scale } })} />
      <label className="observing-inherit"><input type="checkbox" checked={value === null} onChange={e => change({ overrides: { ...draft.overrides, [key]: e.target.checked ? null : policy[key] } })} />Inherit</label>
    </div>;
  };
  return <form onSubmit={e => { e.preventDefault(); if (canWrite && !save.isPending) save.mutate(); }}>
    <fieldset disabled={!canWrite || save.isPending}>
      <legend>{initial.scope === 'global' ? 'Global defaults' : `${initial.scope[0].toUpperCase()}${initial.scope.slice(1)} overrides`}</legend>
      {initial.scope !== 'project' && <label>Scheduling<select aria-label="Scheduling mode" value={draft.enabled === null ? 'inherit' : draft.enabled ? 'preferences' : 'legacy'} onChange={e => change({ enabled: e.target.value === 'inherit' ? null : e.target.value === 'preferences' })}><option value="inherit">Inherit</option><option value="preferences">Observing preferences</option><option value="legacy">Legacy priority</option></select></label>}
      {initial.scope === 'rig' && <label>Planning site<select aria-label="Planning site" value={draft.site_id ?? ''} onChange={e => change({ site_id: e.target.value || null })}><option value="">None</option>{defaults.sites.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}</select></label>}
      <label>Preset<select aria-label="Observing preset" value={preset} onChange={e => {
        setPreset(e.target.value); const chosen = defaults.presets[e.target.value];
        if (chosen) change({ overrides: { ...draft.overrides, weights: { ...chosen.weights }, minimum_dwell_ms: chosen.minimum_dwell_ms, switch_margin: chosen.switch_margin } });
      }}><option value="">Custom</option><option value="balanced">Balanced</option><option value="finish_goals">Finish objectives</option><option value="best_conditions">Best conditions</option></select></label>
      {number('importance', 'Importance', 100)}
      <div className="observing-weights" aria-label="Scoring weights">
        <h4 className="observing-weight-heading">Scoring weights</h4>
        {factors.map(([factor, label]) => { const value = draft.overrides.weights[factor]; return <div className="observing-row" key={factor}>
          <label htmlFor={`weight-${factor}`}>{label}<small>{value === undefined ? sources[factor] ?? 'default' : initial.scope}</small></label>
          <input id={`weight-${factor}`} type="number" min={0} max={1000} step={1} value={value ?? policy.weights[factor]} disabled={value === undefined} onChange={e => change({ overrides: { ...draft.overrides, weights: { ...draft.overrides.weights, [factor]: Number(e.target.value) } } })} />
          <label className="observing-inherit"><input type="checkbox" checked={value === undefined} onChange={e => { const weights = { ...draft.overrides.weights }; if (e.target.checked) delete weights[factor]; else weights[factor] = policy.weights[factor]; change({ overrides: { ...draft.overrides, weights } }); }} />Inherit</label>
        </div>; })}
      </div>
      {number('minimum_dwell_ms', 'Minimum dwell (minutes)', 1440, 60000)}
      {number('switch_margin', 'Switch improvement (%)', 100, 100)}
      <div className="observing-actions"><button type="submit"><Save size={16} />{save.isPending ? 'Saving...' : 'Save preferences'}</button><button type="button" onClick={() => { setPreset(''); change({ overrides: emptyOverrides() }); }}><RotateCcw size={16} />Reset overrides</button></div>
    </fieldset>
    {save.error && <p role="alert">{errorText(reload.error ?? save.error)} <button type="button" disabled={reload.isPending} onClick={() => reload.mutate()}><RefreshCw size={16} />Reload saved preferences</button></p>}
    {saved && <p role="status">Preferences saved.</p>}
  </form>;
}
