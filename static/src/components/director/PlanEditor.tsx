import { type ReactNode, useEffect, useId, useMemo, useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorContribution, DirectorObjective, DirectorPlanDraft, DirectorPlanView, DirectorRigProfileSummary, DirectorTemplate } from '../../api/directorTypes';
import { PURPOSES, bandpassKind, bandpassOptions, convertGoal, coverageGaps, defaultExposure, emptyPlan, formatHours, framesFor, goalExposure, hoursFor, libraryChoice, libraryFor, newContribution, newLibraryContribution, newObjective, panelIds, panelsByRig, planProblem, rigPanels, rigTotals, templateValue, templatesFor } from './planModel';
import './PlanEditor.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Plan request failed';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;

/** What the workspace shows with one rig: where its project lives, and below
 *  its plan, what activation does there and its Target Scheduler rows. */
export interface RigExtras { place?: ReactNode; below?: ReactNode }

/** Objectives per bandpass and depth, and each rig's template and exposure
 *  for them, one block per rig. Rigs with a project in this plan or ticked
 *  come first; the rest fold away. */
export default function PlanEditor({ projectId, linkedRigIds = [], rigExtras, footer }: {
  projectId: string;
  linkedRigIds?: string[];
  rigExtras?: (rig: DirectorRigProfileSummary) => RigExtras;
  footer?: ReactNode;
}) {
  const formId = useId();
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const planKey = ['directorPlan', projectId];
  const loaded = useQuery({ queryKey: planKey, queryFn: () => apiClient.getDirectorPlan(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const rigs = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  // The framing's grid names the panels a rig can own.
  const framing = useQuery({ queryKey: ['directorFraming', projectId], queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const sharedPanels = useMemo(() => panelIds(framing.data?.draft?.mosaic), [framing.data]);
  const rigList = useMemo(() => rigs.data ?? [], [rigs.data]);
  const templateQueries = useQueries({ queries: rigList.map(rig => ({
    queryKey: ['directorTemplates', rig.catalog_slug], queryFn: () => apiClient.getDirectorTemplates(rig.catalog_slug), retry: retryWhenBusy, retryDelay: 700, staleTime: 60_000,
  })) });
  const templatesByRig = useMemo(() => {
    const map: Record<string, DirectorTemplate[]> = {};
    rigList.forEach((rig, index) => { map[rig.rig.id] = templateQueries[index]?.data?.templates ?? []; });
    return map;
  }, [rigList, templateQueries]);
  // A rig framed on its own owns its own grid; the rest share the framing's.
  const panelsFor = useMemo(() => panelsByRig(framing.data?.draft, rigList.map(rig => rig.rig.id)), [framing.data, rigList]);
  const ownFramed = useMemo(() => (framing.data?.draft?.rig_framings ?? []).map(own => own.rig_id), [framing.data]);
  const [plan, setPlan] = useState<DirectorPlanDraft | null>(null);
  // Which rigs the operator has ticked. A rig can take part with no matching
  // template yet, so this is not the same as "has a contribution".
  const [joined, setJoined] = useState<string[]>([]);
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');
  useEffect(() => {
    if (!loaded.data) return;
    const next = loaded.data.plan ?? emptyPlan(projectId);
    setPlan(next);
    // Keep rigs the operator ticked even when a save left them without a contribution.
    setJoined(ids => [...new Set([...ids, ...next.contributions.map(c => c.rig_id)])]);
  }, [loaded.data, projectId]);
  const save = useMutation({
    retry: false,
    mutationFn: () => {
      if (!plan || !loaded.data) throw new Error('Nothing to save');
      return apiClient.saveDirectorPlan({ ...plan, revision: loaded.data.plan?.revision ?? 0 });
    },
    onSuccess: saved => { setNotice(`Saved plan revision ${saved.plan?.revision ?? 0}.`); client.setQueryData<DirectorPlanView>(planKey, saved); },
  });
  const stale = httpStatus(save.error) === 409;
  // Director's own templates: a rig whose database has none for a band shoots with one of these.
  const libraryQuery = useQuery({ queryKey: ['directorTemplateLibrary'], queryFn: apiClient.getDirectorTemplateLibrary, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const library = useMemo(() => libraryQuery.data ?? [], [libraryQuery.data]);
  const options = useMemo(() => bandpassOptions(templatesByRig, library), [templatesByRig, library]);
  const update = (change: (current: DirectorPlanDraft) => DirectorPlanDraft) => setPlan(current => current ? change(current) : current);
  const number = (value: string, fallback: number) => { const parsed = Number(value); return Number.isFinite(parsed) ? parsed : fallback; };

  const addObjective = () => update(current => {
    const used = new Set(current.objectives.map(o => o.bandpass_id));
    const next = options.find(o => !used.has(o.id)) ?? options[0];
    return { ...current, objectives: [...current.objectives, newObjective(next?.id ?? 'luminance')] };
  });
  const changeObjective = (id: string, patch: Partial<DirectorObjective>) => update(current => {
    const objectives = current.objectives.map(o => o.id === id ? { ...o, ...patch } : o);
    // A new bandpass invalidates templates picked for the old one.
    const contributions = patch.bandpass_id
      ? current.contributions.filter(c => c.objective_id !== id)
      : current.contributions;
    return { ...current, objectives, contributions };
  });
  const removeObjective = (id: string) => update(current => ({ ...current, objectives: current.objectives.filter(o => o.id !== id), contributions: current.contributions.filter(c => c.objective_id !== id) }));
  const participating = (rig: DirectorRigProfileSummary) => joined.includes(rig.rig.id);
  const toggleRig = (rig: DirectorRigProfileSummary, on: boolean) => update(current => {
    setJoined(ids => on ? [...new Set([...ids, rig.rig.id])] : ids.filter(id => id !== rig.rig.id));
    if (!on) return { ...current, contributions: current.contributions.filter(c => c.rig_id !== rig.rig.id) };
    const added = current.objectives.flatMap(objective => {
      const matching = templatesFor(objective.bandpass_id, templatesByRig[rig.rig.id] ?? []);
      const template = matching[0];
      if (template) return [newContribution(objective, rig, template, defaultExposure(rig, bandpassKind(objective.bandpass_id, templatesByRig, library), template))];
      // Nothing in the rig's database for this band: the library stands in.
      const shared = libraryFor(objective.bandpass_id, library)[0];
      return shared ? [newLibraryContribution(objective, rig, shared)] : [];
    });
    return { ...current, contributions: [...current.contributions, ...added] };
  });
  const setRigPanels = (rig: DirectorRigProfileSummary, chosen: string[]) => update(current => ({
    ...current,
    // Every panel chosen is the same as no list at all.
    contributions: current.contributions.map(c => c.rig_id === rig.rig.id ? { ...c, panel_ids: chosen.length >= (panelsFor[rig.rig.id] ?? sharedPanels).length ? [] : chosen } : c),
  }));
  const setContribution = (rig: DirectorRigProfileSummary, objective: DirectorObjective, change: (current: DirectorContribution | null) => DirectorContribution | null) => update(current => {
    const existing = current.contributions.find(c => c.rig_id === rig.rig.id && c.objective_id === objective.id) ?? null;
    const next = change(existing);
    const others = current.contributions.filter(c => !(c.rig_id === rig.rig.id && c.objective_id === objective.id));
    return { ...current, contributions: next ? [...others, next] : others };
  });

  // Without a plan the rigs still show where this project lives, so its
  // databases stay reachable when the plan cannot load.
  const waiting = (status: ReactNode) => <section className="plan-editor" aria-label="Acquisition plan">
    {status}
    <section className="plan-rigs" aria-label="Rigs">
      {rigList.filter(rig => linkedRigIds.includes(rig.rig.id)).map(rig => {
        const extras = rigExtras?.(rig) ?? {};
        return <div key={rig.rig.id} className="plan-rig" role="group" aria-label={rig.catalog_name}>
          <p className="plan-rig-head"><strong>{rig.catalog_name}</strong>{extras.place && <span className="plan-rig-place">{extras.place}</span>}</p>
          {extras.below}
        </div>;
      })}
      {footer}
    </section>
  </section>;
  if (loaded.isPending) return waiting(<p role="status">Loading plan...</p>);
  if (loaded.isError) return waiting(<p className="director-error" role="alert">{message(loaded.error)}</p>);
  if (!plan) return null;
  const totals = rigTotals(plan, sharedPanels, panelsFor);
  const gaps = sharedPanels.length > 1 ? coverageGaps(plan, sharedPanels, ownFramed) : [];
  const rigBlock = (rig: DirectorRigProfileSummary) => {
    const index = rigList.indexOf(rig);
    const templates = templatesByRig[rig.rig.id] ?? [];
    const panels = panelsFor[rig.rig.id] ?? sharedPanels;
    const ownGrid = ownFramed.includes(rig.rig.id);
    const loadingTemplates = templateQueries[index]?.isPending;
    const on = participating(rig);
    const total = totals.find(t => t.rigId === rig.rig.id);
    const extras = rigExtras?.(rig) ?? {};
    return <div key={rig.rig.id} className="plan-rig" role="group" aria-label={rig.catalog_name}>
      <fieldset className="plan-rig-plan" disabled={!canWrite || stale}>
      <label className="plan-rig-head"><input type="checkbox" aria-label={`${rig.catalog_name} takes part`} checked={on} onChange={event => toggleRig(rig, event.target.checked)} disabled={plan.objectives.length === 0} />
        <strong>{rig.catalog_name}</strong>
        {extras.place && <span className="plan-rig-place">{extras.place}</span>}
        <small>{rig.field_of_view ? `${rig.field_of_view.pixel_scale_arcsec.toFixed(2)}″/px${rig.field_of_view.focal_ratio ? `, f/${rig.field_of_view.focal_ratio.toFixed(1)}` : ''}` : 'no optics in its rig profile'}{loadingTemplates ? ', loading templates' : `, ${templates.length} template${templates.length === 1 ? '' : 's'}`}</small>
        {total && <span className="plan-rig-total">{total.frames} frames, {formatHours(total.hours)}</span>}
      </label>
      {on && panels.length > 1 && (() => {
        const owned = rigPanels(plan, rig.rig.id, panels);
        return <div className="plan-panels" role="group" aria-label={`${rig.catalog_name} panels`}>
          <span className="director-muted">{ownGrid ? 'Its own panels:' : 'Panels:'}</span>
          <label className="plan-panel"><input type="checkbox" aria-label={`${rig.catalog_name} shoots every panel`} checked={owned.length === panels.length} onChange={event => setRigPanels(rig, event.target.checked ? panels : [])} />All</label>
          {panels.map(id => <label key={id} className="plan-panel"><input type="checkbox" aria-label={`${rig.catalog_name} shoots panel ${id}`} checked={owned.includes(id)} onChange={event => setRigPanels(rig, event.target.checked ? [...owned, id] : owned.filter(p => p !== id))} />{id}</label>)}
          {owned.length === 0 && <span className="director-error">No panel chosen; this rig shoots nothing.</span>}
        </div>;
      })()}
      {on && <table className="plan-contributions"><thead><tr><th>Objective</th><th>Template</th><th>Exposure</th><th>Frames</th><th>On</th></tr></thead><tbody>
        {plan.objectives.map(objective => {
          const contribution = plan.contributions.find(c => c.rig_id === rig.rig.id && c.objective_id === objective.id) ?? null;
          const matching = templatesFor(objective.bandpass_id, templates);
          const label = options.find(o => o.id === objective.bandpass_id)?.name ?? objective.bandpass_id;
          const frames = contribution ? framesFor(objective.goal, contribution.exposure_seconds) : null;
          return <tr key={objective.id}>
            <td>{label}<br /><small className="director-muted">{PURPOSES.find(p => p.id === objective.purpose)?.name ?? objective.purpose}</small></td>
            <td>{(() => {
              const shared = libraryFor(objective.bandpass_id, library);
              if (matching.length === 0 && shared.length === 0) return <span className="director-muted">No {label} template in this database or the library</span>;
              return <select aria-label={`${rig.catalog_name} template for ${label}`} value={templateValue(contribution, library)} onChange={event => {
                const [kind, key] = event.target.value.split(':');
                const own = kind === 'db' ? matching.find(t => String(t.id) === key) : undefined;
                const fromLibrary = kind === 'lib' ? shared.find(t => t.id === key) : undefined;
                setContribution(rig, objective, current => own
                  ? { ...(current ?? newContribution(objective, rig, own, defaultExposure(rig, bandpassKind(objective.bandpass_id, templatesByRig, library), own))), template: { template_guid: own.guid, template_id: own.id, name: own.name, filter_name: own.filter_name, gain: own.gain, offset: own.offset, bin: own.bin, readout_mode: own.readout_mode } }
                  : fromLibrary ? { ...(current ?? newLibraryContribution(objective, rig, fromLibrary)), template: libraryChoice(fromLibrary) }
                  : null);
              }}>
                <option value="">Skip on this rig</option>
                {matching.length > 0 && <optgroup label="In this database">{matching.map(t => <option key={t.id} value={`db:${t.id}`}>{t.name} ({t.filter_name}{t.bin && t.bin > 1 ? `, ${t.bin}×${t.bin}` : ''})</option>)}</optgroup>}
                {shared.length > 0 && <optgroup label="Library, written on activation">{shared.map(t => <option key={t.id} value={`lib:${t.id}`}>{t.name} ({t.filter_name}{t.bin && t.bin > 1 ? `, ${t.bin}×${t.bin}` : ''})</option>)}</optgroup>}
                {contribution && templateValue(contribution, library).startsWith('lib:') && !shared.some(t => t.id === contribution.template.template_guid) && <option value={templateValue(contribution, library)}>{contribution.template.name} (library, since removed)</option>}
              </select>;
            })()}</td>
            <td>{contribution && <span className="plan-goal"><input aria-label={`${rig.catalog_name} exposure for ${label}`} type="number" min={1} step="any" value={contribution.exposure_seconds} onChange={event => setContribution(rig, objective, current => current ? { ...current, exposure_seconds: number(event.target.value, current.exposure_seconds) } : current)} /><small>s</small></span>}</td>
            <td>{contribution && frames !== null && <span data-testid={`frames-${rig.catalog_slug}-${objective.bandpass_id}`}>{frames}{panels.length > 1 && <small className="director-muted"> per panel</small>}<br /><small className="director-muted">{formatHours(hoursFor(frames, contribution.exposure_seconds))}{panels.length > 1 ? ' each' : ''}</small></span>}</td>
            <td>{contribution && <input type="checkbox" aria-label={`${rig.catalog_name} shoots ${label}`} checked={contribution.enabled} onChange={event => setContribution(rig, objective, current => current ? { ...current, enabled: event.target.checked } : current)} />}</td>
          </tr>;
        })}
      </tbody></table>}
      </fieldset>
      {extras.below}
    </div>;
  };
  const inPlan = rigList.filter(rig => participating(rig) || linkedRigIds.includes(rig.rig.id));
  const others = rigList.filter(rig => !inPlan.includes(rig));
  return <section className="plan-editor" aria-label="Acquisition plan">
    <form id={formId} onSubmit={event => { event.preventDefault(); if (!canWrite || save.isPending || stale) return; setNotice(''); const trouble = planProblem(plan); setProblem(trouble ?? ''); if (!trouble) save.mutate(); }}>
      <fieldset disabled={!canWrite || stale}>
        <legend>Objectives</legend>
        <p className="director-muted">Accepted hours or frames per bandpass; hours become frames through each rig's exposure.</p>
        {plan.objectives.length === 0 && <p className="director-muted">No objectives yet.</p>}
        <ul className="plan-objectives">
          {plan.objectives.map(objective => <li key={objective.id} className="plan-objective">
            <label>Bandpass<select aria-label="Objective bandpass" value={objective.bandpass_id} onChange={event => changeObjective(objective.id, { bandpass_id: event.target.value })}>
              {options.map(option => <option key={option.id} value={option.id}>{option.name}{option.kind === 'narrowband' ? ' (narrowband)' : ''}</option>)}
              {!options.some(o => o.id === objective.bandpass_id) && <option value={objective.bandpass_id}>{objective.bandpass_id}</option>}
            </select></label>
            <label>Purpose<select aria-label="Objective purpose" value={objective.purpose} onChange={event => changeObjective(objective.id, { purpose: event.target.value })}>
              {PURPOSES.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
              {!PURPOSES.some(p => p.id === objective.purpose) && <option value={objective.purpose}>{objective.purpose}</option>}
            </select></label>
            <label>Goal<span className="plan-goal">
              <input aria-label="Objective goal" type="number" min={0} step="any" value={objective.goal.value} onChange={event => changeObjective(objective.id, { goal: { ...objective.goal, value: number(event.target.value, 0) } as DirectorObjective['goal'] })} />
              <select aria-label="Objective goal unit" value={objective.goal.kind} title={`Switching units keeps the same goal, read through ${goalExposure(objective, plan, rigList, templatesByRig)} s exposures`}
                onChange={event => changeObjective(objective.id, { goal: convertGoal(objective.goal, event.target.value as 'hours' | 'frames', goalExposure(objective, plan, rigList, templatesByRig)) })}>
                <option value="hours">hours</option><option value="frames">frames per rig</option>
              </select></span></label>
            {canWrite && <button type="button" aria-label={`Remove ${objective.bandpass_id} objective`} title="Remove objective" onClick={() => removeObjective(objective.id)}><Trash2 size={16} /></button>}
          </li>)}
        </ul>
        {canWrite && <button type="button" onClick={addObjective}><Plus size={16} />Add objective</button>}
      </fieldset>
    </form>
    <section className="plan-rigs" aria-label="Rigs">
      <h4 className="plan-rigs-heading">Rigs</h4>
      <p className="director-muted">Tick a rig to shoot the objectives with a template from its database or the shared library.</p>
      {rigs.isError && <p className="director-error" role="alert">Rigs could not be loaded: {message(rigs.error)} <button type="button" onClick={() => void rigs.refetch()}>Retry</button></p>}
      {!rigs.isError && rigList.length === 0 && <p className="director-muted">{rigs.isPending ? 'Loading rigs...' : 'No rig has planning enabled yet.'}</p>}
      {inPlan.map(rigBlock)}
      {others.length > 0 && <details className="plan-other-rigs" open={inPlan.length === 0}>
        <summary>Other rigs ({others.length})</summary>
        {others.map(rigBlock)}
      </details>}
      {footer}
    </section>
      {sharedPanels.length > 1 && <fieldset><legend>Coverage</legend>
        {gaps.length === 0
          ? <p className="director-muted" data-testid="plan-coverage">{plan.objectives.length === 0 ? `${sharedPanels.length} panels, no objectives yet.` : `Every objective has a rig on all ${sharedPanels.length} panels.`}</p>
          : <ul className="plan-gaps" data-testid="plan-coverage">{gaps.map(gap => <li key={gap.objective.id} className="director-error">{options.find(o => o.id === gap.objective.bandpass_id)?.name ?? gap.objective.bandpass_id}: no rig on {gap.panels.length === sharedPanels.length ? 'any panel' : `panel${gap.panels.length === 1 ? '' : 's'} ${gap.panels.join(', ')}`}.</li>)}</ul>}
      </fieldset>}
      {notice && <p role="status">{notice}</p>}
      {stale && <p className="director-error" role="alert">This plan changed since you loaded it. Reload to see the saved plan before editing again.</p>}
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      <div className="director-actions">
        {canWrite && <button type="submit" form={formId} disabled={save.isPending || stale}><Check size={16} />{save.isPending ? 'Saving...' : 'Save plan'}</button>}
        <button type="button" aria-label="Reload plan" title="Reload plan" onClick={() => { setNotice(''); setProblem(''); save.reset(); void loaded.refetch(); }}><RefreshCw size={16} /></button>
        {!canWrite && <span className="director-muted">Read only</span>}
      </div>
  </section>;
}
