import type { NightAvailabilityState, NightInterval, NightRuntime, RigNight, RigNights } from '../../api/nightPlanTypes';

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

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
    const usable = Math.max(0, target.hours_up_available - target.hours_moon_blocked);
    const fits = Math.min(usable, owed.get(`${target.project_id}:${target.target_id}`) ?? 0);
    if (fits > 0) hours.set(target.project_id, (hours.get(target.project_id) ?? 0) + fits);
  }
  return rig.plans
    .filter(plan => hours.has(plan.project_id))
    .map(plan => ({ project_id: plan.project_id, name: plan.name, hours: hours.get(plan.project_id)! }))
    .sort((a, b) => b.hours - a.hours || a.name.localeCompare(b.name));
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
