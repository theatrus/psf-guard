import type { DirectorScheduleGoal, NightInterval, NightOverride, OverrideSubject, ScheduleBlock, ScheduledNight } from '../../api/nightPlanTypes';
import { formatClock } from './nightsModel';

/** What an override can name: the plans, targets and exposure plans of a
 *  Director schedule's goals, each once. */
export interface OverrideSubjects {
  plans: Array<{ subject: OverrideSubject; name: string }>;
  targets: Array<{ subject: OverrideSubject; name: string }>;
  exposurePlans: Array<{ subject: OverrideSubject; name: string }>;
}

export function subjectsOf(goals: DirectorScheduleGoal[]): OverrideSubjects {
  const plans = new Map<string, string>();
  const targets = new Map<string, string>();
  const exposurePlans = new Map<string, string>();
  for (const goal of goals) {
    plans.set(goal.plan_id, goal.plan_name);
    targets.set(goal.target_guid, goal.target_name);
    exposurePlans.set(goal.exposureplan_guid, `${goal.target_name} · ${goal.filter ?? 'exposure plan'}`);
  }
  const sorted = <T extends { name: string }>(items: T[]) => items.sort((a, b) => a.name.localeCompare(b.name));
  return {
    plans: sorted([...plans].map(([plan_id, name]) => ({ subject: { kind: 'plan', plan_id } as const, name }))),
    targets: sorted([...targets].map(([target_guid, name]) => ({ subject: { kind: 'target', target_guid } as const, name }))),
    exposurePlans: sorted([...exposurePlans].map(([exposureplan_guid, name]) => ({ subject: { kind: 'exposure_plan', exposureplan_guid } as const, name }))),
  };
}

/** A subject as one string, for a select's value. */
export function subjectKey(subject: OverrideSubject): string {
  switch (subject.kind) {
    case 'plan': return `plan:${subject.plan_id}`;
    case 'target': return `target:${subject.target_guid}`;
    case 'exposure_plan': return `exposure_plan:${subject.exposureplan_guid}`;
  }
}

export function subjectFromKey(key: string): OverrideSubject | null {
  const [kind, id] = key.split(':');
  if (!id) return null;
  if (kind === 'plan') return { kind, plan_id: id };
  if (kind === 'target') return { kind, target_guid: id };
  if (kind === 'exposure_plan') return { kind, exposureplan_guid: id };
  return null;
}

/** The goals an override's subject covers. */
export function goalsOf(subject: OverrideSubject, goals: DirectorScheduleGoal[]): DirectorScheduleGoal[] {
  return goals.filter(goal => {
    switch (subject.kind) {
      case 'plan': return goal.plan_id === subject.plan_id;
      case 'target': return goal.target_guid === subject.target_guid;
      case 'exposure_plan': return goal.exposureplan_guid === subject.exposureplan_guid;
    }
  });
}

const KIND_WORD: Record<OverrideSubject['kind'], string> = {
  plan: 'a plan',
  target: 'a target',
  exposure_plan: 'an exposure plan',
};

/** A subject's name, or what it was when nothing on the rig has it now. */
export function subjectName(subject: OverrideSubject, goals: DirectorScheduleGoal[]): string {
  const goal = goalsOf(subject, goals)[0];
  if (!goal) return `${KIND_WORD[subject.kind]} the rig has no work for`;
  switch (subject.kind) {
    case 'plan': return goal.plan_name;
    case 'target': return goal.target_name;
    case 'exposure_plan': return `${goal.target_name} · ${goal.filter ?? 'exposure plan'}`;
  }
}

function span(window: NightInterval, zone?: string): string {
  return `${formatClock(window.start_ms, zone)} to ${formatClock(window.end_ms, zone)}`;
}

/** An override in words, such as `Only IC 1805 from 21:00 to 23:00`. */
export function describeOverride(entry: NightOverride, goals: DirectorScheduleGoal[], zone?: string): string {
  switch (entry.kind) {
    case 'pin': return `Only ${subjectName(entry.subject, goals)} from ${span(entry.window, zone)}`;
    case 'exclude': return `Leave out ${subjectName(entry.subject, goals)} ${entry.window ? `from ${span(entry.window, zone)}` : 'all night'}`;
    case 'hold': return entry.from_ms === undefined ? 'Hold the rig all night' : `Hold the rig from ${formatClock(entry.from_ms, zone)}`;
    case 'order': {
      const names = entry.plans.map(plan_id => subjectName({ kind: 'plan', plan_id }, goals));
      return `Plans in this order: ${names.join(', ')}`;
    }
  }
}

/** A stretch of the timeline an override covers. */
export interface OverrideBand {
  start_ms: number;
  end_ms: number;
  kind: 'pin' | 'exclude' | 'hold';
  label: string;
}

/** The timeline's bands for a night's overrides. An exclusion without a
 *  window covers the whole night, so it is listed, not drawn. */
export function overrideBands(entries: NightOverride[], goals: DirectorScheduleGoal[], night: NightInterval, zone?: string): OverrideBand[] {
  const bands: OverrideBand[] = [];
  for (const entry of entries) {
    const label = describeOverride(entry, goals, zone);
    if (entry.kind === 'pin') bands.push({ ...entry.window, kind: 'pin', label });
    if (entry.kind === 'exclude' && entry.window) bands.push({ ...entry.window, kind: 'exclude', label });
    if (entry.kind === 'hold') bands.push({ start_ms: Math.max(entry.from_ms ?? night.start_ms, night.start_ms), end_ms: night.end_ms, kind: 'hold', label });
  }
  return bands;
}

/** Pins whose subject has no frames planned inside their window: the sky
 *  or the Moon rules it out there, so the window stays idle. A subject the
 *  rig has no work for left is not pinned at all, so it never idles. */
export function idlePins(entries: NightOverride[], scheduled: ScheduledNight, goals: DirectorScheduleGoal[]): NightInterval[] {
  const idle: NightInterval[] = [];
  for (const entry of entries) {
    if (entry.kind !== 'pin') continue;
    const covered = goalsOf(entry.subject, goals);
    if (covered.length === 0) continue;
    const plans = new Set(covered.map(goal => goal.exposure_plan_id));
    const runs = scheduled.blocks.flatMap(block => block.runs)
      .some(run => plans.has(run.exposure_plan_id) && run.start_ms < entry.window.end_ms && run.end_ms > entry.window.start_ms);
    if (!runs) idle.push(entry.window);
  }
  return idle;
}

/** The target a block shoots, as an override names it. */
export function blockSubject(block: ScheduleBlock, goals: DirectorScheduleGoal[]): OverrideSubject | null {
  const goal = goals.find(g => g.target_id === block.target.target_id && g.project_id === block.target.project_id);
  return goal ? { kind: 'target', target_guid: goal.target_guid } : null;
}

/**
 * The overrides that move a block by `delta_ms`: its target pinned to the
 * new stretch, and left out of the part of the old one the move uncovers,
 * so it does not simply run there first. `null` when the block's target is
 * unknown or the move leaves the night.
 */
export function moveBlock(block: ScheduleBlock, deltaMs: number, goals: DirectorScheduleGoal[], night: NightInterval): NightOverride[] | null {
  const subject = blockSubject(block, goals);
  if (!subject || deltaMs === 0) return null;
  const window = { start_ms: block.start_ms + deltaMs, end_ms: block.end_ms + deltaMs };
  if (window.start_ms < night.start_ms || window.end_ms > night.end_ms) return null;
  const uncovered = deltaMs > 0
    ? { start_ms: block.start_ms, end_ms: Math.min(window.start_ms, block.end_ms) }
    : { start_ms: Math.max(window.end_ms, block.start_ms), end_ms: block.end_ms };
  return [
    { kind: 'pin', subject, window },
    { kind: 'exclude', subject, window: uncovered },
  ];
}
