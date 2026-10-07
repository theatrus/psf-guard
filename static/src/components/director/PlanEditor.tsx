import { type MutableRefObject, type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useDraftSection } from './pageDraftsState';
import { describePlanChanges } from './draftChanges';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Plus, RefreshCw, Trash2 } from 'lucide-react';
import NumberInput from '../NumberInput';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorContribution, DirectorGoal, DirectorObjective, DirectorPlanDraft, DirectorPlanView, DirectorRigProfileSummary, DirectorTemplate } from '../../api/directorTypes';
import { PURPOSES, bandpassKind, bandpassOptions, convertGoal, coverageGaps, goalFor, speedAdjustedHours, defaultExposure, emptyPlan, formatHours, framesFor, goalExposure, hoursFor, libraryChoice, libraryFor, newContribution, newLibraryContribution, newObjective, onePartEach, panelIds, panelsByRig, planProblem, rigPanels, rigTotals, samePlan, shootingRigs, templateValue, templatesFor } from './planModel';
import './PlanEditor.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Plan request failed';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;

/** What the workspace shows with one rig: where its project lives, and below
 *  its plan, what activation does there and its Target Scheduler rows. */
export interface RigExtras { place?: ReactNode; below?: ReactNode }

/** What became of a request to add or drop a rig: whether it now shoots
 *  the plan (or stopped), and in a few words what was left out or why
 *  nothing changed. */
export interface RigChange { done: boolean; note: string | null }

/** What the Rigs tab asks of the plan: which rigs shoot it, and a way to
 *  add or drop one, filling in a template for each objective as a tick does. */
export interface PlanRigControls {
  setRig: (rigId: string, on: boolean) => RigChange;
}

/** What the plan editor tells the page: the rigs shooting the plan and its
 *  objective count, saved or not, and whether rigs can join yet. */
export interface PlanRigState { rigIds: string[]; objectives: number; ready: boolean }

const orList = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} or ${words[words.length - 1]}`;

/** Objectives per bandpass and depth, and each rig's template and exposure
 *  for them, one block per rig. Rigs with a project in this plan or ticked
 *  come first; the rest fold away. With `controls`, rigs join and leave
 *  from elsewhere on the page, and only the rigs in the plan are shown. */
export default function PlanEditor({ projectId, linkedRigIds = [], rigExtras, footer, controls, onRigsChange }: {
  projectId: string;
  linkedRigIds?: string[];
  rigExtras?: (rig: DirectorRigProfileSummary) => RigExtras;
  footer?: ReactNode;
  controls?: MutableRefObject<PlanRigControls | null>;
  onRigsChange?: (state: PlanRigState) => void;
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
  // The saved copy the draft started from: what it is compared with, and
  // whose revision a save names.
  const [base, setBase] = useState<DirectorPlanDraft | null>(null);
  // Rigs shown with the plan: those shooting it when it loaded and those
  // added since. A rig whose objectives are all switched off stays in view
  // until it is dropped, so unticking one does not take its block away.
  const [kept, setKept] = useState<string[]>([]);
  // A copy saved elsewhere arrived over unsaved edits.
  const [conflict, setConflict] = useState(false);
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');
  const take = useCallback((saved: DirectorPlanDraft) => {
    const next = onePartEach(saved);
    setPlan(next); setBase(next); setKept(shootingRigs(next)); setConflict(false);
  }, []);
  const latest = useRef({ plan, base });
  latest.current = { plan, base };
  useEffect(() => {
    if (!loaded.data) return;
    const next = loaded.data.plan ?? emptyPlan(projectId);
    const { plan: draft, base: was } = latest.current;
    // The server's copy replaces the draft only while nothing is unsaved.
    // Over unsaved edits a newer copy (another browser's save, a plan an
    // attach brought in) is a conflict to settle, never a silent reset.
    if (!draft || !was || samePlan(draft, was)) take(next);
    else if (next.revision !== was.revision) setConflict(true);
  }, [loaded.data, projectId, take]);
  const save = useMutation({
    retry: false,
    mutationFn: (sent: DirectorPlanDraft) => apiClient.saveDirectorPlan({ ...sent, revision: latest.current.base?.revision ?? 0 }),
    onSuccess: (saved, sent) => {
      const next = saved.plan ?? emptyPlan(projectId);
      setNotice(`Saved plan revision ${next.revision}.`);
      // Edits made while the save ran stay unsaved on top of the new copy.
      setBase(next);
      setPlan(current => !current || samePlan(current, sent) ? next : current);
      client.setQueryData<DirectorPlanView>(planKey, saved);
    },
    onError: error => { if (httpStatus(error) === 409) { setConflict(true); void loaded.refetch(); } },
  });
  const stale = conflict || httpStatus(save.error) === 409;
  const unsaved = !!plan && !!base && !samePlan(plan, base);
  // Drop the draft for the newest saved copy, asking first when that loses edits.
  const reload = () => {
    if (canWrite && unsaved && !window.confirm('Drop the unsaved exposure changes and load the saved plan?')) return;
    setNotice(''); setProblem(''); save.reset();
    void loaded.refetch().then(result => { if (result.data) take(result.data.plan ?? emptyPlan(projectId)); });
  };
  // On the project page the save bar saves the plan with the framing;
  // activation reads the saved plan, so nothing here may look applied
  // before it is saved.
  const managed = useDraftSection('plan', {
    label: 'Exposures',
    order: 2,
    unsaved: canWrite && unsaved,
    changes: describePlanChanges(base, plan, id => rigList.find(rig => rig.rig.id === id)?.catalog_name ?? 'a rig'),
    save: async () => {
      if (!unsaved) return true;
      if (stale) return 'the plan changed elsewhere; reload it first';
      if (!canWrite || !plan) return false;
      const trouble = planProblem(plan);
      setProblem(trouble ?? '');
      if (trouble) return trouble;
      setNotice('');
      try { await save.mutateAsync(plan); } catch (error) { return httpStatus(error) === 409 ? 'the plan changed elsewhere; reload it first' : message(error); }
      return true;
    },
    // The newest saved copy, which also settles a conflict.
    discard: () => { const saved = loaded.data ? loaded.data.plan ?? emptyPlan(projectId) : base; if (saved) take(saved); setProblem(''); save.reset(); },
  });
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
  // A rig shoots the plan when one of its contributions is on.
  const shootingKey = shootingRigs(plan).join(',');
  const shooting = useMemo(() => shootingKey ? shootingKey.split(',') : [], [shootingKey]);
  const participating = (rig: DirectorRigProfileSummary) => kept.includes(rig.rig.id) || shooting.includes(rig.rig.id);
  const bandName = (id: string) => options.find(o => o.id === id)?.name ?? id;
  const toggleRig = (rig: DirectorRigProfileSummary, on: boolean): RigChange => {
    if (!plan) return { done: false, note: 'plan still loading' };
    const id = rig.rig.id;
    if (!on) {
      // Switched off, not deleted: activation sets the rig's Target
      // Scheduler project inactive from them, and turning it on again
      // brings the same templates and exposures back.
      update(current => ({ ...current, contributions: current.contributions.map(c => c.rig_id === id ? { ...c, enabled: false } : c) }));
      setKept(ids => ids.filter(other => other !== id));
      return { done: true, note: null };
    }
    if (plan.objectives.length === 0) return { done: false, note: 'add an objective first' };
    if (templateQueries[rigList.indexOf(rig)]?.isPending || libraryQuery.isPending) return { done: false, note: 'templates still loading' };
    // Contributions the rig already has come back on; only the objectives
    // it lacks get one, so a rig never shoots an objective twice.
    const owned = new Set(plan.contributions.filter(c => c.rig_id === id).map(c => c.objective_id));
    const lacking: string[] = [];
    const added = plan.objectives.filter(objective => !owned.has(objective.id)).flatMap(objective => {
      const template = templatesFor(objective.bandpass_id, templatesByRig[id] ?? [])[0];
      if (template) return [newContribution(objective, rig, template, defaultExposure(rig, bandpassKind(objective.bandpass_id, templatesByRig, library), template))];
      // Nothing in the rig's database for this band: the library stands in.
      const shared = libraryFor(objective.bandpass_id, library)[0];
      if (shared) return [newLibraryContribution(objective, rig, shared)];
      lacking.push(bandName(objective.bandpass_id));
      return [];
    });
    const gap = lacking.length > 0 ? `no ${orList(lacking)} template` : null;
    if (owned.size === 0 && added.length === 0) return { done: false, note: gap };
    update(current => {
      const has = new Set(current.contributions.filter(c => c.rig_id === id).map(c => c.objective_id));
      return { ...current, contributions: [...current.contributions.map(c => c.rig_id === id ? { ...c, enabled: true } : c), ...added.filter(c => !has.has(c.objective_id))] };
    });
    setKept(ids => [...new Set([...ids, id])]);
    return { done: true, note: gap };
  };
  const objectiveCount = plan?.objectives.length ?? 0;
  const ready = !!plan && rigs.isSuccess;
  useEffect(() => { onRigsChange?.({ rigIds: shooting, objectives: objectiveCount, ready }); }, [shooting, objectiveCount, ready, onRigsChange]);
  // Refreshed after every render, so the Rigs tab always adds with the
  // templates and objectives as they stand.
  useEffect(() => {
    if (!controls) return;
    controls.current = { setRig: (rigId, on) => {
      const rig = rigList.find(entry => entry.rig.id === rigId);
      return rig ? toggleRig(rig, on) : { done: false, note: rigs.isPending ? 'rigs still loading' : 'not a planning rig' };
    } };
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
    // Templates the server left out (Moon rules it cannot use), by name.
    const templateWarnings = templateQueries[index]?.data?.warnings ?? [];
    const on = participating(rig);
    const total = totals.find(t => t.rigId === rig.rig.id);
    const extras = rigExtras?.(rig) ?? {};
    return <div key={rig.rig.id} className="plan-rig" role="group" aria-label={rig.catalog_name}>
      <fieldset className="plan-rig-plan" disabled={!canWrite || stale}>
      <label className="plan-rig-head">{!controls && <input type="checkbox" aria-label={`${rig.catalog_name} takes part`} checked={on} onChange={event => { const change = toggleRig(rig, event.target.checked); setNotice(change.note ? `${rig.catalog_name}: ${change.note}` : ''); }} disabled={plan.objectives.length === 0} />}
        <strong>{rig.catalog_name}</strong>
        {extras.place && <span className="plan-rig-place">{extras.place}</span>}
        <small>{rig.field_of_view ? `${rig.field_of_view.pixel_scale_arcsec.toFixed(2)}″/px${rig.field_of_view.focal_ratio ? `, f/${rig.field_of_view.focal_ratio.toFixed(1)}` : ''}, ` : ''}{loadingTemplates ? 'loading templates' : `${templates.length} template${templates.length === 1 ? '' : 's'}`}</small>
        {total && <span className="plan-rig-total">{total.frames} frames, {formatHours(total.hours)}</span>}
        {on && !shooting.includes(rig.rig.id) && <span className="director-muted">not shooting</span>}
      </label>
      {templateWarnings.length > 0 && <p className="director-muted" role="note" aria-label={`${rig.catalog_name} templates left out`}>{templateWarnings.join(' ')}</p>}
      {on && panels.length > 1 && (() => {
        const owned = rigPanels(plan, rig.rig.id, panels);
        return <div className="plan-panels" role="group" aria-label={`${rig.catalog_name} panels`}>
          <span className="director-muted">{ownGrid ? 'Separate panels:' : 'Panels:'}</span>
          <label className="plan-panel"><input type="checkbox" aria-label={`${rig.catalog_name} shoots every panel`} checked={owned.length === panels.length} onChange={event => setRigPanels(rig, event.target.checked ? panels : [])} />All</label>
          {panels.map(id => <label key={id} className="plan-panel"><input type="checkbox" aria-label={`${rig.catalog_name} shoots panel ${id}`} checked={owned.includes(id)} onChange={event => setRigPanels(rig, event.target.checked ? [...owned, id] : owned.filter(p => p !== id))} />{id}</label>)}
          {owned.length === 0 && <span className="director-error">No panel chosen; this rig shoots nothing.</span>}
        </div>;
      })()}
      {on && <table className="plan-contributions"><thead><tr><th>Objective</th><th>Template</th><th>Exposure</th><th>Goal</th><th>Frames</th><th>On</th></tr></thead><tbody>
        {plan.objectives.map(objective => {
          const contribution = plan.contributions.find(c => c.rig_id === rig.rig.id && c.objective_id === objective.id) ?? null;
          const matching = templatesFor(objective.bandpass_id, templates);
          const label = options.find(o => o.id === objective.bandpass_id)?.name ?? objective.bandpass_id;
          const goal = contribution ? goalFor(contribution, objective) : objective.goal;
          const frames = contribution ? framesFor(goal, contribution.exposure_seconds) : null;
          const suggested = contribution && !contribution.goal ? speedAdjustedHours(objective.goal, rig.field_of_view?.focal_ratio) : null;
          const setGoal = (next: DirectorGoal | null) => setContribution(rig, objective, current => current ? { ...current, goal: next } : current);
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
            <td>{contribution && <span className="plan-goal"><NumberInput aria-label={`${rig.catalog_name} exposure for ${label}`} min={1} step="any" value={contribution.exposure_seconds} onChange={event => setContribution(rig, objective, current => current ? { ...current, exposure_seconds: number(event.target.value, current.exposure_seconds) } : current)} /><small>s</small></span>}</td>
            <td className="plan-rig-goal">{contribution && (contribution.goal
              ? <span className="plan-goal">
                  <NumberInput aria-label={`${rig.catalog_name} goal for ${label}`} min={0} step="any" value={contribution.goal.value} onChange={event => setGoal({ ...contribution.goal!, value: number(event.target.value, contribution.goal!.value) } as DirectorGoal)} />
                  <select aria-label={`${rig.catalog_name} goal unit for ${label}`} value={contribution.goal.kind} onChange={event => setGoal(convertGoal(contribution.goal!, event.target.value as 'hours' | 'frames', contribution.exposure_seconds))}>
                    <option value="hours">h</option><option value="frames">frames</option>
                  </select>
                  <button type="button" className="link-button" aria-label={`${rig.catalog_name} uses the plan's goal for ${label}`} title="Use the plan's goal" onClick={() => setGoal(null)}>×</button>
                </span>
              : <span className="director-muted">{objective.goal.kind === 'hours' ? `${objective.goal.value} h` : `${objective.goal.value} frames`}
                  {canWrite && <> <button type="button" className="link-button" onClick={() => setGoal(objective.goal)}>Set</button></>}
                  {canWrite && suggested !== null && <> <button type="button" className="link-button" title={`At f/${rig.field_of_view!.focal_ratio!.toFixed(1)}, about ${suggested} h reaches f/5's depth`} onClick={() => setGoal({ kind: 'hours', value: suggested })}>f/{rig.field_of_view!.focal_ratio!.toFixed(1)}: {suggested} h</button></>}
                </span>)}</td>
            <td>{contribution && frames !== null && <span data-testid={`frames-${rig.catalog_slug}-${objective.bandpass_id}`}>{frames}{panels.length > 1 && <small className="director-muted"> per panel</small>}<br /><small className="director-muted">{formatHours(hoursFor(frames, contribution.exposure_seconds))}{panels.length > 1 ? ' each' : ''}</small></span>}</td>
            <td>{contribution && <input type="checkbox" aria-label={`${rig.catalog_name} shoots ${label}`} checked={contribution.enabled} onChange={event => setContribution(rig, objective, current => current ? { ...current, enabled: event.target.checked } : current)} />}</td>
          </tr>;
        })}
      </tbody></table>}
      </fieldset>
      {extras.below}
    </div>;
  };
  // Rigs join from the Rigs tab when it is there; this lists only those
  // shooting the plan.
  const inPlan = rigList.filter(rig => participating(rig) || (!controls && linkedRigIds.includes(rig.rig.id)));
  const others = controls ? [] : rigList.filter(rig => !inPlan.includes(rig));
  return <section className="plan-editor" aria-label="Acquisition plan">
    <form id={formId} onSubmit={event => { event.preventDefault(); if (!canWrite || save.isPending || stale) return; setNotice(''); const trouble = planProblem(plan); setProblem(trouble ?? ''); if (!trouble) save.mutate(plan); }}>
      <fieldset disabled={!canWrite || stale}>
        <legend>Objectives</legend>
        <p className="director-muted">Hours or frames per filter</p>
        {plan.objectives.length === 0 && <p className="director-muted">No objectives yet</p>}
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
              <NumberInput aria-label="Objective goal" min={0} step="any" value={objective.goal.value} onChange={event => changeObjective(objective.id, { goal: { ...objective.goal, value: number(event.target.value, 0) } as DirectorObjective['goal'] })} />
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
    <section className="plan-rigs" aria-label={controls ? 'Exposures per rig' : 'Rigs'}>
      <h4 className="plan-rigs-heading">{controls ? 'Each rig' : 'Rigs'}</h4>
      <p className="director-muted">{controls ? 'Add or drop rigs on the Rigs tab' : 'Tick a rig to shoot the objectives with a template from its database or the shared library.'}</p>
      {controls && !rigs.isPending && inPlan.length === 0 && <p className="director-muted">No rigs yet</p>}
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
      {stale && <p className="director-error" role="alert">This plan changed since you loaded it. <button type="button" onClick={reload}>Reload</button></p>}
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      <div className="director-actions">
        {canWrite && !managed && <button type="submit" form={formId} disabled={save.isPending || stale}><Check size={16} />{save.isPending ? 'Saving...' : 'Save plan'}</button>}
        <button type="button" aria-label="Reload plan" title="Reload plan" onClick={reload}><RefreshCw size={16} /></button>
        {!canWrite && <span className="director-muted">Read only</span>}
      </div>
  </section>;
}
