import type { NightAvailabilityState, NightInterval, NightPlanProject, NightPlanTarget, NightRuntime, NightTargetHours, NightSchedule, RigNight, RigNights, SchedulePlanProgress } from '../../api/nightPlanTypes';
import { withoutPlanningParams } from '../../hooks/useUrlState';

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

/** The Rigs page's URL keys: which tab, and the Nights grid's range,
 *  hidden rigs and chosen night. They stay on the Rigs page. */
export const RIGS_TAB_PARAM = 'rigsTab';
export const NIGHTS_PARAMS = {
  from: 'nightsFrom',
  count: 'nightsCount',
  hide: 'nightsHide',
  rig: 'nightRig',
  night: 'night',
} as const;

/** An address that keeps the page's database and project scope, less the
 *  keys that belong to one view, with `extra` added. */
export function scopedHref(path: string, search: string, extra: Record<string, string> = {}): string {
  const params = withoutPlanningParams(search);
  for (const [key, value] of Object.entries(extra)) params.set(key, value);
  const query = params.toString();
  return query ? `${path}?${query}` : path;
}

/** The Rigs page's address for one rig's night, keeping the scope. */
export function nightHref(rigId: string, date: string, search = ''): string {
  return scopedHref('/rigs', search, { [RIGS_TAB_PARAM]: 'nights', [NIGHTS_PARAMS.rig]: rigId, [NIGHTS_PARAMS.night]: date });
}

/** Night counts the page offers. */
export const NIGHT_COUNTS = [7, 14, 31] as const;
/** How many plan colours the grid's palette has. */
export const PLAN_COLOURS = 8;

export const RUNTIME_LABEL: Record<NightRuntime, string> = {
  target_scheduler: 'Target Scheduler',
  director: 'Director',
};

export const STATE_LABEL: Record<NightAvailabilityState, string> = {
  available: 'Available',
  unavailable: 'Not available',
  limited: 'Limited',
};

/** A local calendar date as `YYYY-MM-DD`. */
export function localDate(at: Date): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

/** Whether text is a `YYYY-MM-DD` date. */
export function isDate(text: string | null): text is string {
  if (!text || !/^\d{4}-\d{2}-\d{2}$/.test(text)) return false;
  const [year, month, day] = text.split('-').map(Number);
  const parsed = new Date(year, month - 1, day);
  return parsed.getFullYear() === year && parsed.getMonth() === month - 1 && parsed.getDate() === day;
}

/** The instant the server reads as the night of `date`: that evening, in
 *  the browser's clock. The server keys nights by the site's local mean
 *  noon, which this falls well after. */
export function eveningOf(date: string): number {
  const [year, month, day] = date.split('-').map(Number);
  return new Date(year, month - 1, day, 20, 0, 0, 0).getTime();
}

/** `Mon 3 Nov` for a night's date. */
export function formatNightDate(date: string): string {
  const [year, month, day] = date.split('-').map(Number);
  return new Date(year, month - 1, day).toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short' });
}

export function formatClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

/** `HH:MM` in the browser's clock, for a time field. */
export function clockValue(ms: number): string {
  const at = new Date(ms);
  return `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`;
}

/**
 * The instant a clock time falls on in a night: from noon on the evening's
 * date to noon the next day, so 01:30 is after midnight.
 */
export function clockOnNight(date: string, clock: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(clock);
  if (!match) return null;
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  if (hours > 23 || minutes > 59) return null;
  const [year, month, day] = date.split('-').map(Number);
  return new Date(year, month - 1, hours < 12 ? day + 1 : day, hours, minutes).getTime();
}

/**
 * A limited night's window from two clock times: an empty `from` starts at
 * dusk and an empty `to` ends at dawn. The window is clipped to the night,
 * noon to noon. `null` when nothing is left.
 */
export function windowFor(night: Pick<RigNight, 'date' | 'noon_ms' | 'dusk_ms' | 'dawn_ms'>, from: string, to: string): NightInterval | null {
  const start = from ? clockOnNight(night.date, from) : night.dusk_ms;
  const end = to ? clockOnNight(night.date, to) : night.dawn_ms;
  if (start === null || end === null) return null;
  const clippedStart = Math.max(start, night.noon_ms);
  const clippedEnd = Math.min(end, night.noon_ms + DAY_MS);
  return clippedEnd > clippedStart ? { start_ms: clippedStart, end_ms: clippedEnd } : null;
}

/** Where the dark part of a night sits in a strip from dusk to dawn: the
 *  available windows as fractions of it. A night without darkness is
 *  empty. */
export function availableShares(night: RigNight): Array<{ from: number; to: number }> {
  if (night.dusk_ms === null || night.dawn_ms === null || night.dawn_ms <= night.dusk_ms) return [];
  const span = night.dawn_ms - night.dusk_ms;
  const share = (window: NightInterval) => ({
    from: Math.max(0, (window.start_ms - night.dusk_ms!) / span),
    to: Math.min(1, (window.end_ms - night.dusk_ms!) / span),
  });
  switch (night.availability.state) {
    case 'available':
      return [{ from: 0, to: 1 }];
    case 'unavailable':
      return [];
    case 'limited':
      return night.availability.windows.map(share).filter(part => part.to > part.from);
  }
}

/** The glyph a cell shows for its mark: ✕ out, ◐ limited, ✓ marked clear,
 *  nothing when unmarked. */
export function markGlyph(night: RigNight): string {
  if (night.availability.state === 'unavailable') return '✕';
  if (night.availability.state === 'limited') return '◐';
  return night.availability.marks.length > 0 ? '✓' : '';
}

/** A night's load in a few words: idle hours, or the hours overbooked. */
export function loadWords(night: RigNight): string {
  const { demand_hours, idle_hours, overbooked_hours, available_hours } = night.load;
  if (available_hours <= 0) return demand_hours > 0 ? 'no time' : '';
  if (demand_hours <= 0) return 'idle';
  if (overbooked_hours >= 0.05) return `+${overbooked_hours.toFixed(1)} h over`;
  if (idle_hours >= 0.05) return `${idle_hours.toFixed(1)} h idle`;
  return 'full';
}

/** The plans a night has work for, by the hours each could use. */
export function plansOnNight(rig: RigNights, night: RigNight): Array<{ project_id: number; name: string; hours: number }> {
  const owed = new Map<string, number>();
  for (const plan of rig.plans) for (const target of plan.targets) owed.set(`${plan.project_id}:${target.target_id}`, target.hours_owed);
  const hours = new Map<number, number>();
  for (const target of night.targets) {
    const fits = Math.min(usableHours(target), owed.get(`${target.project_id}:${target.target_id}`) ?? 0);
    if (fits > 0) hours.set(target.project_id, (hours.get(target.project_id) ?? 0) + fits);
  }
  return rig.plans
    .filter(plan => hours.has(plan.project_id))
    .map(plan => ({ project_id: plan.project_id, name: plan.name, hours: hours.get(plan.project_id)! }))
    .sort((a, b) => b.hours - a.hours || a.name.localeCompare(b.name));
}

/** The hours a target is up, clear and allowed by the Moon on a night. */
export function usableHours(hours: NightTargetHours | undefined): number {
  return hours ? Math.max(0, hours.hours_up_available - hours.hours_moon_blocked) : 0;
}

/** Where an owed target stands on one night. */
export type NightTargetStatus =
  /** The night's schedule shoots it. */
  | 'planned'
  /** Up, clear and allowed by the Moon; the schedule shoots other work, or
   *  there is no schedule to say. */
  | 'usable'
  /** Its plan is done on an earlier night of the schedule. */
  | 'done'
  /** Up while the night is clear, but its filters' Moon rules turn it down. */
  | 'moon'
  /** Up only while the night is marked not clear. */
  | 'not_clear'
  /** Not up inside its meridian window in the dark. */
  | 'outside_window'
  /** Not above its horizon and limits in the dark. */
  | 'below_limits'
  /** The night has no darkness. */
  | 'no_darkness';

/** Why a target the night does not apply to is out, but for `done`, whose
 *  reason names the night. */
export const OUT_REASON: Record<Exclude<NightTargetStatus, 'planned' | 'usable' | 'done'>, string> = {
  moon: 'the Moon rules of its filters turn it down',
  not_clear: 'up only while the night is marked not clear',
  outside_window: 'not up in its meridian window in the dark',
  below_limits: 'not above its horizon and limits in the dark',
  no_darkness: 'no darkness this night',
};

export interface PlanRow {
  plan: NightPlanProject;
  target: NightPlanTarget;
  hours: NightTargetHours | undefined;
  /** Frames the shown schedule plans for the target this night. */
  planned: number;
  /** When its first block this night starts, for order. */
  first_ms: number | null;
  /** Hours it is up, clear and allowed by the Moon. */
  usable: number;
  /** The scheduled night its plan is done on, when it is. */
  done_on: string | null;
  status: NightTargetStatus;
}

/** The index of the scheduled night a target's exposure plans are all
 *  done on: -1 when they were done before the first, null when not within
 *  the schedule. */
export function completesOn(progress: SchedulePlanProgress[]): number | null {
  if (progress.length === 0) return null;
  const owed = progress.filter(p => !p.complete_before);
  if (owed.length === 0) return -1;
  if (owed.some(p => p.completes_on === null)) return null;
  return Math.max(...owed.map(p => p.completes_on!));
}

/**
 * A rig's owed targets on one night, split into those the night applies
 * to (planned first, in the schedule's order, then the most usable) and
 * the rest, each with why it is out. `schedule` is the one shown for the
 * night, if any.
 */
export function planRows(rig: RigNights, night: RigNight, schedule: NightSchedule | null): { applies: PlanRow[]; out: PlanRow[] } {
  const index = schedule ? schedule.nights.findIndex(n => n.date === night.date) : -1;
  const scheduled = index >= 0 ? schedule!.nights[index] : null;
  const byTarget = new Map(night.targets.map(hours => [`${hours.project_id}:${hours.target_id}`, hours]));
  const rows: PlanRow[] = rig.plans.flatMap(plan => plan.targets.map(target => {
    const hours = byTarget.get(`${plan.project_id}:${target.target_id}`);
    const progress = schedule?.progress.filter(p => p.project_id === plan.project_id && p.target_id === target.target_id) ?? [];
    const planned = index >= 0 ? progress.reduce((sum, p) => sum + (p.planned[index] ?? 0), 0) : 0;
    const first = scheduled?.blocks.find(block => block.target.project_id === plan.project_id && block.target.target_id === target.target_id);
    const done = index >= 0 ? completesOn(progress) : null;
    const usable = usableHours(hours);
    const status: NightTargetStatus = planned > 0 ? 'planned'
      : done !== null && done < index ? 'done'
      : usable > 0.01 ? 'usable'
      : (hours?.hours_up_available ?? 0) > 0.01 ? 'moon'
      : (hours?.hours_up ?? 0) > 0.01 ? 'not_clear'
      : night.dark_hours <= 0 ? 'no_darkness'
      : plan.limits.meridian_window_minutes > 0 ? 'outside_window'
      : 'below_limits';
    return {
      plan, target, hours, planned, usable, status,
      first_ms: first?.start_ms ?? null,
      done_on: done !== null && done >= 0 ? schedule!.nights[done]?.date ?? null : null,
    };
  }));
  const byName = (a: PlanRow, b: PlanRow) => a.plan.name.localeCompare(b.plan.name) || a.target.name.localeCompare(b.target.name);
  const applies = rows
    .filter(row => row.status === 'planned' || row.status === 'usable')
    .sort((a, b) => (a.first_ms ?? Infinity) - (b.first_ms ?? Infinity) || b.planned - a.planned || b.usable - a.usable || byName(a, b));
  const out = rows.filter(row => row.status !== 'planned' && row.status !== 'usable').sort(byName);
  return { applies, out };
}

/** A colour slot for each plan, stable across the page: by name, so one
 *  plan shot by two rigs keeps its colour. */
export function planColours(rigs: RigNights[]): Map<string, number> {
  const names = [...new Set(rigs.flatMap(rig => rig.plans.map(plan => plan.name)))].sort((a, b) => a.localeCompare(b));
  return new Map(names.map((name, index) => [name, index % PLAN_COLOURS]));
}

/** The dates from one to another in a rig's nights, both included, in
 *  order, whichever comes first. */
export function datesBetween(rig: RigNights, a: string, b: string): string[] {
  const dates = rig.nights.map(night => night.date);
  const from = dates.indexOf(a);
  const to = dates.indexOf(b);
  if (from < 0 || to < 0) return [];
  return dates.slice(Math.min(from, to), Math.max(from, to) + 1);
}

/** `Mon 3 Nov` or `Mon 3 – Thu 6 Nov`, for a set of nights. */
export function describeDates(dates: string[]): string {
  if (dates.length === 0) return '';
  if (dates.length === 1) return formatNightDate(dates[0]);
  return `${formatNightDate(dates[0])} – ${formatNightDate(dates[dates.length - 1])} (${dates.length} nights)`;
}

/** Whether a night is the one under way now. */
export function isTonight(night: Pick<RigNight, 'noon_ms'>, now: number): boolean {
  return now >= night.noon_ms && now < night.noon_ms + DAY_MS;
}

/** A dark span's length in hours, for words. */
export function spanHours(night: Pick<RigNight, 'dusk_ms' | 'dawn_ms'>): number {
  return night.dusk_ms !== null && night.dawn_ms !== null ? (night.dawn_ms - night.dusk_ms) / HOUR_MS : 0;
}

/** Whether a scheduled night ends with every plan done. */
export function scheduleDone(rig: RigNights, date: string): boolean {
  return rig.schedule?.nights.find(n => n.date === date)?.gaps.some(gap => gap.reason === 'complete') ?? false;
}

/** The plans a scheduled night shoots, by planned exposure hours. */
export function plansScheduled(rig: RigNights, date: string): Array<{ name: string; hours: number }> {
  const night = rig.schedule?.nights.find(n => n.date === date);
  if (!night) return [];
  const hours = new Map<string, number>();
  for (const block of night.blocks) {
    const exposure = block.runs.reduce((sum, run) => sum + run.frames * run.exposure_seconds, 0) / 3600;
    hours.set(block.target.project_name, (hours.get(block.target.project_name) ?? 0) + exposure);
  }
  return [...hours].map(([name, value]) => ({ name, hours: value })).sort((a, b) => b.hours - a.hours || a.name.localeCompare(b.name));
}
