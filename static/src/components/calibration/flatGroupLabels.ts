import type { FlatChangeEvent, FlatClockRun, FlatFamily, FlatGroupsRig, FlatSession, FlatSplit } from '../../api/types';

/** A difference in percent: two decimals below 1%, one above. */
export function percent(value: number): string {
  return `${value >= 1 ? value.toFixed(1) : value.toFixed(2)}%`;
}

/** A limit as a person set it: `1%`, `0.5%`. */
export function limitPercent(value: number): string {
  return `${Math.round(value * 100) / 100}%`;
}

/** A rotator angle, `54°`, or `no rotator`. */
export function angleLabel(rotation: number | null): string {
  return rotation === null ? 'no rotator' : `${Math.round(rotation * 10) / 10}°`;
}

/**
 * A Unix time on a clock, `2026-09-18 08:30`: `timeZone`, the viewer's
 * when absent.
 */
function clock(seconds: number, timeZone?: string): string {
  const parts = new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone,
  }).formatToParts(new Date(seconds * 1000));
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')} ${part('hour')}:${part('minute')}`;
}

/** The viewer's zone at that time, `PDT` in summer and `PST` in winter. */
function zone(seconds: number, timeZone?: string): string {
  return new Intl.DateTimeFormat('en-US', { timeZoneName: 'short', timeZone })
    .formatToParts(new Date(seconds * 1000))
    .find((part) => part.type === 'timeZoneName')?.value ?? '';
}

/** `UTC−7`, `UTC+5:30`, `UTC`. */
function offsetName(seconds: number): string {
  if (seconds === 0) return 'UTC';
  const minutes = Math.abs(seconds) / 60;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return `UTC${seconds < 0 ? '−' : '+'}${hours}${rest ? `:${String(rest).padStart(2, '0')}` : ''}`;
}

/** The rig's clock at a time: the run it falls in, or the first. */
function rigOffset(seconds: number, runs: readonly FlatClockRun[]): number {
  let offset = runs[0].offset_seconds;
  for (const run of runs) {
    if (run.from <= seconds) offset = run.offset_seconds;
  }
  return offset;
}

/** A time's clock and zone: on the rig's clock when its frames record one,
 *  else on `timeZone`, the viewer's when absent. */
function parts(seconds: number, runs?: readonly FlatClockRun[], timeZone?: string): [string, string] {
  if (runs && runs.length > 0) {
    const offset = rigOffset(seconds, runs);
    return [clock(seconds + offset, 'UTC'), offsetName(offset)];
  }
  return [clock(seconds, timeZone), zone(seconds, timeZone)];
}

/**
 * A Unix time with its zone: on the rig's clock, as its flats were shot by,
 * `2026-10-12 06:30 UTC−7`, or on the viewer's, `2026-10-12 06:30 PDT`.
 */
export function timeLabel(seconds: number, runs?: readonly FlatClockRun[], timeZone?: string): string {
  return parts(seconds, runs, timeZone).join(' ');
}

/**
 * The window a change happened in, `2026-09-18 08:30 – 2026-09-21 12:28 UTC−7`,
 * on the clock [`timeLabel`] reads; each end names its zone when the clocks
 * changed in between.
 */
export function windowLabel(start: number, end: number, runs?: readonly FlatClockRun[], timeZone?: string): string {
  const [[startClock, from], [endClock, to]] = [parts(start, runs, timeZone), parts(end, runs, timeZone)];
  return from === to
    ? `${startClock} – ${endClock} ${to}`
    : `${startClock} ${from} – ${endClock} ${to}`;
}

/** `2026-09-07 – 2026-09-16`, or one night. */
export function nightRange(first: string, last: string): string {
  return first === last ? first : `${first} – ${last}`;
}

/** Two nights of one year without the year, `09-07 → 09-16`; one night alone. */
export function nightSpan(first: string, last: string): string {
  const short = first.slice(0, 4) === last.slice(0, 4) && /^\d{4}-/.test(first);
  const trim = (night: string) => (short ? night.slice(5) : night);
  return first === last ? trim(first) : `${trim(first)} → ${trim(last)}`;
}

/** A group as a session in the list names it: `Group 2 · 09-07 → 09-16, 2 nights`. */
export function groupLabel(number: number, group: { first_night: string; last_night: string; sessions: unknown[] }): string {
  return `Group ${number} · ${nightSpan(group.first_night, group.last_night)}, ${plural(group.sessions.length, 'night')}`;
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** A family's heading: `R · 54°`. */
export function familyLabel(family: Pick<FlatFamily, 'filter' | 'rotation'>): string {
  return `${family.filter ?? 'No filter'} · ${angleLabel(family.rotation)}`;
}

/** Where a change sits: on the camera, seen by every filter, or on a filter. */
export function eventSideLabel(event: FlatChangeEvent): string {
  if (event.camera_side) return 'All filters and angles: camera side';
  return event.filters.length === 1
    ? `Only ${event.filters[0]}: on the filter`
    : `Only ${event.filters.join(', ')}`;
}

/** The filters and angles that saw a change: `R, G, B at 3°, 54°`. */
export function eventSeenIn(event: FlatChangeEvent): string {
  const filters = event.filters.length > 0 ? event.filters.join(', ') : 'no filter';
  const angles = event.rotations.map(angleLabel).join(', ');
  return angles ? `${filters} at ${angles}` : filters;
}

/** Every session of a rig, by its response key. */
export function sessionsByKey(rigs: FlatGroupsRig[]): Map<string, FlatSession> {
  const sessions = new Map<string, FlatSession>();
  for (const rig of rigs) {
    for (const family of rig.families) {
      for (const group of family.groups) {
        for (const session of group.sessions) sessions.set(session.response_key, session);
      }
    }
  }
  return sessions;
}

/** What two sessions set side by side show: the change or split between them. */
export function findPair(
  rigs: FlatGroupsRig[],
  earlier: string,
  later: string,
): { event?: FlatChangeEvent; split?: FlatSplit } {
  for (const rig of rigs) {
    const event = rig.events.find((entry) => entry.before_response === earlier && entry.after_response === later);
    if (event) return { event };
  }
  for (const rig of rigs) {
    for (const family of rig.families) {
      const split = family.splits.find((entry) => entry.before_response === earlier && entry.after_response === later);
      if (split) return { split };
    }
  }
  return {};
}
