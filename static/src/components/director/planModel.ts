import type { DirectorContribution, DirectorGoal, DirectorObjective, DirectorPlanDraft, DirectorRigProfileSummary, DirectorTemplate, DirectorTemplateChoice } from '../../api/directorTypes';

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

export function bandpassOptions(templatesByRig: Record<string, DirectorTemplate[]>) {
  const options = new Map(KNOWN_BANDPASSES.map(b => [b.id, b]));
  for (const templates of Object.values(templatesByRig)) {
    for (const template of templates) {
      if (!options.has(template.bandpass.id)) options.set(template.bandpass.id, { id: template.bandpass.id, name: template.bandpass.name === 'Custom' ? template.filter_name : template.bandpass.name, kind: template.bandpass.kind });
    }
  }
  return [...options.values()];
}

export function bandpassKind(id: string, templatesByRig: Record<string, DirectorTemplate[]>): 'broadband' | 'narrowband' {
  return bandpassOptions(templatesByRig).find(b => b.id === id)?.kind ?? 'broadband';
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
  return { template_guid: template.guid, template_id: template.id, name: template.name, filter_name: template.filter_name, gain: template.gain, offset: template.offset, bin: template.bin, readout_mode: template.readout_mode };
}

/** The template's own default when it has one, else the rig's for the band kind. */
export function defaultExposure(rig: DirectorRigProfileSummary, kind: 'broadband' | 'narrowband', template: DirectorTemplate | null): number {
  if (template && template.default_exposure > 0) return template.default_exposure;
  return kind === 'narrowband' ? rig.default_exposure_seconds.narrowband : rig.default_exposure_seconds.broadband;
}

export function newContribution(objective: DirectorObjective, rig: DirectorRigProfileSummary, template: DirectorTemplate, exposure: number): DirectorContribution {
  return { id: newId(), objective_id: objective.id, rig_id: rig.rig.id, template: choiceFrom(template), exposure_seconds: exposure, panel_ids: [], enabled: true };
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

export function formatHours(hours: number): string {
  if (hours < 1) return `${Math.round(hours * 60)} min`;
  return `${hours.toFixed(hours >= 10 ? 0 : 1)} h`;
}

export interface RigTotal { rigId: string; frames: number; hours: number }

export function rigTotals(plan: DirectorPlanDraft): RigTotal[] {
  const byRig = new Map<string, RigTotal>();
  for (const contribution of plan.contributions) {
    if (!contribution.enabled) continue;
    const objective = plan.objectives.find(o => o.id === contribution.objective_id);
    if (!objective) continue;
    const frames = framesFor(objective.goal, contribution.exposure_seconds) ?? 0;
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
