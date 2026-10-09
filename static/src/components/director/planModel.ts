import type { DirectorContribution, DirectorGoal, DirectorLibraryTemplate, DirectorMoonPolicy, DirectorMosaic, DirectorObjective, DirectorPlanDraft, DirectorRigProfileSummary, DirectorTemplate, DirectorTemplateChoice, DirectorFramingDraft} from '../../api/directorTypes';

export const PURPOSES: Array<{ id: string; name: string }> = [
  { id: 'faint_detail', name: 'Faint detail' },
  { id: 'unsaturated_stars', name: 'Unsaturated stars' },
];

/** Bandpasses offered before any template says otherwise; the core's ids. */
export const KNOWN_BANDPASSES: Array<{ id: string; name: string; kind: 'broadband' | 'narrowband' }> = [
  { id: 'luminance', name: 'Luminance', kind: 'broadband' },
  { id: 'red', name: 'Red', kind: 'broadband' },
  { id: 'green', name: 'Green', kind: 'broadband' },
  { id: 'blue', name: 'Blue', kind: 'broadband' },
  { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' },
  { id: 'oiii', name: 'O III', kind: 'narrowband' },
  { id: 'sii', name: 'S II', kind: 'narrowband' },
];

export function bandpassOptions(templatesByRig: Record<string, DirectorTemplate[]>, library: DirectorLibraryTemplate[] = []) {
  const options = new Map(KNOWN_BANDPASSES.map(b => [b.id, b]));
  for (const template of [...Object.values(templatesByRig).flat(), ...library]) {
    if (!options.has(template.bandpass.id)) options.set(template.bandpass.id, { id: template.bandpass.id, name: template.bandpass.name === 'Custom' ? template.filter_name : template.bandpass.name, kind: template.bandpass.kind });
  }
  return [...options.values()];
}

export function bandpassKind(id: string, templatesByRig: Record<string, DirectorTemplate[]>, library: DirectorLibraryTemplate[] = []): 'broadband' | 'narrowband' {
  return bandpassOptions(templatesByRig, library).find(b => b.id === id)?.kind ?? 'broadband';
}

/** The library templates for a bandpass. */
export function libraryFor(bandpassId: string, library: DirectorLibraryTemplate[]): DirectorLibraryTemplate[] {
  return library.filter(template => template.bandpass.id === bandpassId);
}

/** A library template as a plan binds to it: by its library GUID and its
 *  settings, with no row id, since the rig database has no such row yet. */
export function libraryChoice(template: DirectorLibraryTemplate): DirectorTemplateChoice {
  return { template_guid: template.id, template_id: null, name: template.name, filter_name: template.filter_name, gain: template.gain, offset: template.offset, bin: template.bin, readout_mode: template.readout_mode, moon: template.moon };
}

export function newLibraryContribution(objective: DirectorObjective, rig: DirectorRigProfileSummary, template: DirectorLibraryTemplate): DirectorContribution {
  return { id: newId(), objective_id: objective.id, rig_id: rig.rig.id, template: libraryChoice(template), exposure_seconds: template.default_exposure_seconds, panel_ids: [], enabled: true };
}

const sameMoon = (wanted: DirectorMoonPolicy | undefined, found: DirectorMoonPolicy | undefined) =>
  !wanted || (!!found && wanted.enabled === found.enabled && wanted.separation_degrees === found.separation_degrees
    && wanted.width_days === found.width_days && wanted.relax_degrees_per_degree === found.relax_degrees_per_degree
    && wanted.relax_min_altitude_degrees === found.relax_min_altitude_degrees
    && wanted.relax_max_altitude_degrees === found.relax_max_altitude_degrees && wanted.moon_down === found.moon_down);

/** The rig's own template a library choice already names: the row
 *  activation wrote under the library GUID, or one with the same filter,
 *  camera settings and Moon rules. Activation picks that row rather than
 *  writing the library's (`resolve_template`), so the library adds nothing
 *  for this rig. */
export function ownTwin(choice: DirectorTemplateChoice, templates: DirectorTemplate[]): DirectorTemplate | undefined {
  const filter = (name: string) => name.trim().toLowerCase();
  const unset = (value: number | null | undefined) => value === null || value === undefined || value < 0 ? -1 : value;
  const bin = (value: number | null | undefined) => value === null || value === undefined || value < 1 ? 1 : value;
  const fits = (template: DirectorTemplate) => filter(template.filter_name) === filter(choice.filter_name) && sameMoon(choice.moon, template.moon);
  const guid = choice.template_guid?.toLowerCase();
  return (guid ? templates.find(template => template.guid?.toLowerCase() === guid && fits(template)) : undefined)
    ?? templates.find(template => fits(template) && unset(template.gain) === unset(choice.gain) && unset(template.offset) === unset(choice.offset)
      && bin(template.bin) === bin(choice.bin) && unset(template.readout_mode) === unset(choice.readout_mode));
}

/** The plan with each library choice a rig's database already holds bound
 *  to that row instead, keeping its exposure; the same object when nothing
 *  changes. */
export function bindOwnTemplates(plan: DirectorPlanDraft, templatesByRig: Record<string, DirectorTemplate[]>): DirectorPlanDraft {
  let changed = false;
  const contributions = plan.contributions.map(contribution => {
    if (contribution.template.template_id !== null) return contribution;
    const twin = ownTwin(contribution.template, templatesByRig[contribution.rig_id] ?? []);
    if (!twin) return contribution;
    changed = true;
    return { ...contribution, template: choiceFrom(twin) };
  });
  return changed ? { ...plan, contributions } : plan;
}

/** What the template control shows for a contribution: the rig's own row
 *  (`db:<id>`), a library template (`lib:<id>`), or nothing. */
export function templateValue(contribution: DirectorContribution | null, library: DirectorLibraryTemplate[]): string {
  if (!contribution) return '';
  if (contribution.template.template_id !== null) return `db:${contribution.template.template_id}`;
  if (contribution.template.template_guid && library.some(t => t.id === contribution.template.template_guid)) return `lib:${contribution.template.template_guid}`;
  return contribution.template.template_guid ? `lib:${contribution.template.template_guid}` : '';
}

export function newId(): string {
  return crypto.randomUUID();
}

export function newObjective(bandpass_id: string): DirectorObjective {
  return { id: newId(), bandpass_id, purpose: 'faint_detail', goal: { kind: 'hours', value: 4 }, priority: 1 };
}

export function templatesFor(bandpassId: string, templates: DirectorTemplate[]): DirectorTemplate[] {
  return templates.filter(template => template.bandpass.id === bandpassId);
}

export function choiceFrom(template: DirectorTemplate): DirectorTemplateChoice {
  return { template_guid: template.guid, template_id: template.id, name: template.name, filter_name: template.filter_name, gain: template.gain, offset: template.offset, bin: template.bin, readout_mode: template.readout_mode, moon: template.moon };
}

/** The template's own default when it has one, else the rig's for the band kind. */
export function defaultExposure(rig: DirectorRigProfileSummary, kind: 'broadband' | 'narrowband', template: DirectorTemplate | null): number {
  if (template && template.default_exposure > 0) return template.default_exposure;
  return kind === 'narrowband' ? rig.default_exposure_seconds.narrowband : rig.default_exposure_seconds.broadband;
}

export function newContribution(objective: DirectorObjective, rig: DirectorRigProfileSummary, template: DirectorTemplate, exposure: number): DirectorContribution {
  return { id: newId(), objective_id: objective.id, rig_id: rig.rig.id, template: choiceFrom(template), exposure_seconds: exposure, panel_ids: [], enabled: true };
}

/** The goal a rig works to: its own when set, else the objective's. */
export function goalFor(contribution: DirectorContribution, objective: DirectorObjective): DirectorGoal {
  return contribution.goal ?? objective.goal;
}

/** Hours that reach an hours goal's depth on a rig at this focal ratio,
 *  read as hours at f/5 (signal per pixel goes as 1 / f-ratio²), to the
 *  half hour. Null when there is nothing to adjust: a frames goal, an
 *  unknown ratio, or a rig within a quarter of f/5's speed. */
export function speedAdjustedHours(goal: DirectorGoal, focalRatio: number | null | undefined): number | null {
  if (goal.kind !== 'hours' || !(focalRatio && focalRatio > 0) || !(goal.value > 0)) return null;
  const factor = (focalRatio / 5) ** 2;
  if (factor > 0.8 && factor < 1.25) return null;
  return Math.max(0.5, Math.round(goal.value * factor * 2) / 2);
}

/** Accepted frames one rig owes an objective at its exposure length. */
export function framesFor(goal: DirectorGoal, exposureSeconds: number): number | null {
  if (!(exposureSeconds > 0)) return null;
  if (goal.kind === 'frames') return goal.value > 0 ? Math.round(goal.value) : null;
  return goal.value > 0 ? Math.ceil((goal.value * 3600) / exposureSeconds) : null;
}

export function hoursFor(frames: number, exposureSeconds: number): number {
  return (frames * exposureSeconds) / 3600;
}

/** The exposure length a goal is read through when it changes unit: the
 *  first rig already shooting the objective, else the first rig's default
 *  for the band, else a plain 300 s narrowband or 120 s broadband. */
export function goalExposure(objective: DirectorObjective, plan: DirectorPlanDraft, rigs: DirectorRigProfileSummary[], templatesByRig: Record<string, DirectorTemplate[]>): number {
  const shooting = plan.contributions.find(c => c.objective_id === objective.id && c.enabled && c.exposure_seconds > 0);
  if (shooting) return shooting.exposure_seconds;
  const kind = bandpassKind(objective.bandpass_id, templatesByRig);
  const rig = rigs[0];
  if (rig) {
    const exposure = defaultExposure(rig, kind, templatesFor(objective.bandpass_id, templatesByRig[rig.rig.id] ?? [])[0] ?? null);
    if (exposure > 0) return exposure;
  }
  return kind === 'narrowband' ? 300 : 120;
}

/** The same goal in the other unit, so switching units keeps the meaning:
 *  6 h at 300 s is 72 frames, and 72 frames at 300 s is 6 h. */
export function convertGoal(goal: DirectorGoal, kind: DirectorGoal['kind'], exposureSeconds: number): DirectorGoal {
  if (goal.kind === kind) return goal;
  if (kind === 'frames') return { kind, value: framesFor(goal, exposureSeconds) ?? 0 };
  return { kind, value: Math.round(hoursFor(goal.value, exposureSeconds) * 100) / 100 };
}

export function formatHours(hours: number): string {
  if (hours < 1) return `${Math.round(hours * 60)} min`;
  return `${hours.toFixed(hours >= 10 ? 0 : 1)} h`;
}

export interface RigTotal { rigId: string; frames: number; hours: number }

export function rigTotals(plan: DirectorPlanDraft, panels: string[] = [], byRigPanels: Record<string, string[]> = {}): RigTotal[] {
  const byRig = new Map<string, RigTotal>();
  for (const contribution of plan.contributions) {
    if (!contribution.enabled) continue;
    const objective = plan.objectives.find(o => o.id === contribution.objective_id);
    if (!objective) continue;
    const own = byRigPanels[contribution.rig_id] ?? panels;
    const frames = (framesFor(goalFor(contribution, objective), contribution.exposure_seconds) ?? 0) * (own.length > 1 ? panelFactor(contribution, own) : 1);
    const total = byRig.get(contribution.rig_id) ?? { rigId: contribution.rig_id, frames: 0, hours: 0 };
    total.frames += frames;
    total.hours += hoursFor(frames, contribution.exposure_seconds);
    byRig.set(contribution.rig_id, total);
  }
  return [...byRig.values()];
}

export function emptyPlan(projectId: string): DirectorPlanDraft {
  return { project_id: projectId, revision: 0, objectives: [], contributions: [], updated_at_ms: 0 };
}

/** The rigs that shoot a plan: those with an enabled contribution. A rig
 *  dropped from the plan keeps its contributions, switched off, so
 *  activation can set its Target Scheduler project inactive. */
export function shootingRigs(plan: DirectorPlanDraft | null | undefined): string[] {
  return [...new Set((plan?.contributions ?? []).filter(c => c.enabled).map(c => c.rig_id))];
}

/** The plan with one contribution per rig and objective, the first. The
 *  server refuses a second one; a plan saved before it did keeps loading,
 *  and its next save drops the repeat. */
export function onePartEach(plan: DirectorPlanDraft): DirectorPlanDraft {
  const seen = new Set<string>();
  const contributions = plan.contributions.filter(c => {
    const part = `${c.rig_id}:${c.objective_id}`;
    if (seen.has(part)) return false;
    seen.add(part);
    return true;
  });
  return contributions.length === plan.contributions.length ? plan : { ...plan, contributions };
}

/** Whether two copies of a plan ask for the same work, whatever their
 *  revision and save time. */
export function samePlan(left: DirectorPlanDraft, right: DirectorPlanDraft): boolean {
  return JSON.stringify([left.objectives, left.contributions]) === JSON.stringify([right.objectives, right.contributions]);
}

/** Name the first thing that cannot be sent, or null when the plan is sound. */
export function planProblem(plan: DirectorPlanDraft): string | null {
  for (const objective of plan.objectives) {
    if (!/^[a-z0-9_]{1,64}$/.test(objective.bandpass_id)) return 'Choose a bandpass for every objective.';
    if (!(objective.goal.value > 0)) return 'Every objective needs a goal above zero.';
  }
  for (const contribution of plan.contributions) {
    if (!(contribution.exposure_seconds > 0)) return 'Every rig exposure must be above zero seconds.';
  }
  return null;
}

/** Panel ids the way the core names them: row one at the top, column one east. */
export function panelIds(mosaic: DirectorMosaic | null | undefined): string[] {
  if (!mosaic) return [];
  const ids: string[] = [];
  for (let r = 1; r <= mosaic.rows; r++) for (let c = 1; c <= mosaic.columns; c++) ids.push(`r${r}c${c}`);
  return ids;
}

/** The panels each rig can own: its own grid when it is framed on its own,
 *  else the shared grid. */
export function panelsByRig(draft: DirectorFramingDraft | null | undefined, rigIds: string[]): Record<string, string[]> {
  const shared = panelIds(draft?.mosaic);
  const map: Record<string, string[]> = {};
  for (const rigId of rigIds) {
    const own = draft?.rig_framings?.find(entry => entry.rig_id === rigId);
    map[rigId] = own ? panelIds(own.mosaic) : shared;
  }
  return map;
}

/** The panels a rig owns: the union over its contributions; empty means all. */
export function rigPanels(plan: DirectorPlanDraft, rigId: string, panels: string[]): string[] {
  const own = plan.contributions.filter(c => c.rig_id === rigId);
  if (own.length === 0 || own.some(c => c.panel_ids.length === 0)) return panels;
  return panels.filter(id => own.some(c => c.panel_ids.includes(id)));
}

/** Per objective, the panels no enabled contribution covers. */
export function coverageGaps(plan: DirectorPlanDraft, panels: string[], ownFramed: string[] = []): Array<{ objective: DirectorObjective; panels: string[] }> {
  // A rig framed on its own covers its own grid, not the shared one.
  const shared = plan.contributions.filter(c => !ownFramed.includes(c.rig_id));
  return plan.objectives.map(objective => ({
    objective,
    panels: panels.filter(panel => !shared.some(c => c.enabled && c.objective_id === objective.id && (c.panel_ids.length === 0 || c.panel_ids.includes(panel)))),
  })).filter(gap => gap.panels.length > 0);
}

/** Frames a rig owes across its panels, for the per-rig total. */
export function panelFactor(contribution: DirectorContribution, panels: string[]): number {
  return contribution.panel_ids.length === 0 ? Math.max(1, panels.length) : contribution.panel_ids.filter(id => panels.includes(id)).length;
}
