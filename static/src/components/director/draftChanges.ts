import type { DirectorContribution, DirectorGoal, DirectorPlanDraft } from '../../api/directorTypes';
import { formatDegrees, type FramingState } from './framingModel';
import { KNOWN_BANDPASSES, PURPOSES } from './planModel';

/** What the save bar says changed: a few words per change, against the
 *  state the section loaded with. */

const angle = (value: number) => `${Math.round(value * 10) / 10}°`;
const same = (left: unknown, right: unknown) => JSON.stringify(left) === JSON.stringify(right);

/** Angular distance between two sky positions, in arcminutes. */
function separationArcmin(a: { ra_degrees: number; dec_degrees: number }, b: { ra_degrees: number; dec_degrees: number }): number {
  const rad = Math.PI / 180;
  const cos = Math.sin(a.dec_degrees * rad) * Math.sin(b.dec_degrees * rad)
    + Math.cos(a.dec_degrees * rad) * Math.cos(b.dec_degrees * rad) * Math.cos((a.ra_degrees - b.ra_degrees) * rad);
  return Math.acos(Math.min(1, Math.max(-1, cos))) / rad * 60;
}

export function describeFramingChanges(before: FramingState | null, after: FramingState | null, rigName: (id: string) => string = id => id): string[] {
  if (!before || !after) return [];
  const changes: string[] = [];
  if (before.targetName !== after.targetName) changes.push(`target name “${before.targetName}” → “${after.targetName}”`);
  if (!same(before.center, after.center)) {
    const moved = separationArcmin(before.center, after.center);
    changes.push(moved < 1 ? `target moved ${Math.round(moved * 60)}″` : `target moved ${Math.round(moved * 10) / 10}′`);
  }
  if (before.positionAngle !== after.positionAngle) changes.push(`camera angle ${angle(before.positionAngle)} → ${angle(after.positionAngle)}`);
  if (!same(before.mosaic, after.mosaic)) {
    const grid = (m: FramingState['mosaic']) => `${m.rows}×${m.columns}, ${m.overlap_percent}% overlap`;
    changes.push(`mosaic ${grid(before.mosaic)} → ${grid(after.mosaic)}`);
  }
  if (before.panelRigId !== after.panelRigId) changes.push(`panel rig ${before.panelRigId ? rigName(before.panelRigId) : 'none'} → ${after.panelRigId ? rigName(after.panelRigId) : 'none'}`);
  else if (!same(before.panel, after.panel)) {
    const size = (p: FramingState['panel']) => p ? `${formatDegrees(p.width_degrees)} × ${formatDegrees(p.height_degrees)}` : 'none';
    changes.push(`panel size ${size(before.panel)} → ${size(after.panel)}`);
  }
  if (!same([...before.shownRigIds].sort(), [...after.shownRigIds].sort())) changes.push('rigs compared');
  if (before.surveyId !== after.surveyId) changes.push('sky survey');
  const framed = (state: FramingState) => new Map(state.rigFramings.map(own => [own.rig_id, own]));
  const was = framed(before);
  const now = framed(after);
  for (const [id, own] of now) {
    if (!was.has(id)) changes.push(`${rigName(id)} framed separately`);
    else if (!same(was.get(id), own)) changes.push(`${rigName(id)} framing`);
  }
  for (const id of was.keys()) if (!now.has(id)) changes.push(`${rigName(id)} back to shared`);
  return changes;
}

const bandpassName = (id: string) => KNOWN_BANDPASSES.find(entry => entry.id === id)?.name ?? id;
const purposeName = (id: string) => PURPOSES.find(entry => entry.id === id)?.name ?? id;
const goalText = (goal: DirectorGoal) => goal.kind === 'hours' ? `${goal.value} h` : `${goal.value} frames per rig`;

export function describePlanChanges(before: DirectorPlanDraft | null, after: DirectorPlanDraft | null, rigName: (id: string) => string = id => id): string[] {
  if (!before || !after) return [];
  const changes: string[] = [];
  const objectiveName = (plan: DirectorPlanDraft, id: string) => bandpassName(plan.objectives.find(objective => objective.id === id)?.bandpass_id ?? id);
  const was = new Map(before.objectives.map(objective => [objective.id, objective]));
  const now = new Map(after.objectives.map(objective => [objective.id, objective]));
  for (const [id, objective] of now) {
    const old = was.get(id);
    const name = bandpassName(objective.bandpass_id);
    if (!old) { changes.push(`${name} added, ${goalText(objective.goal)}`); continue; }
    if (old.bandpass_id !== objective.bandpass_id) changes.push(`${bandpassName(old.bandpass_id)} → ${name}`);
    if (!same(old.goal, objective.goal)) changes.push(`${name} goal ${goalText(old.goal)} → ${goalText(objective.goal)}`);
    if (old.purpose !== objective.purpose) changes.push(`${name} for ${purposeName(objective.purpose).toLowerCase()}`);
    if (old.priority !== objective.priority) changes.push(`${name} priority ${old.priority} → ${objective.priority}`);
  }
  for (const [id, objective] of was) if (!now.has(id)) changes.push(`${bandpassName(objective.bandpass_id)} removed`);
  // A rig's part in an objective is keyed by the pair, not the row id: a
  // rig ticked off and on again is the same part.
  const key = (contribution: DirectorContribution) => `${contribution.rig_id}:${contribution.objective_id}`;
  const parts = (plan: DirectorPlanDraft) => new Map(plan.contributions.map(contribution => [key(contribution), contribution]));
  const oldParts = parts(before);
  const newParts = parts(after);
  for (const [part, contribution] of newParts) {
    const old = oldParts.get(part);
    const label = `${rigName(contribution.rig_id)} ${objectiveName(after, contribution.objective_id)}`;
    if (!old) { changes.push(`${label} added`); continue; }
    if (old.enabled !== contribution.enabled) changes.push(`${label} ${contribution.enabled ? 'on' : 'off'}`);
    if (old.exposure_seconds !== contribution.exposure_seconds) changes.push(`${label} exposure ${old.exposure_seconds} s → ${contribution.exposure_seconds} s`);
    if (!same(old.template, contribution.template)) changes.push(`${label} template ${old.template.name} → ${contribution.template.name}`);
    if (!same([...old.panel_ids].sort(), [...contribution.panel_ids].sort())) changes.push(`${label} panels`);
  }
  for (const [part, contribution] of oldParts) {
    if (!newParts.has(part)) changes.push(`${rigName(contribution.rig_id)} ${objectiveName(before, contribution.objective_id)} removed`);
  }
  return changes;
}
