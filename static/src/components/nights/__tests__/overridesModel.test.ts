import { describe, expect, it } from 'vitest';
import type { DirectorScheduleGoal, ScheduleBlock, ScheduledNight } from '../../../api/nightPlanTypes';
import { formatClock } from '../nightsModel';
import { describeOverride, idlePins, moveBlock, overrideBands, subjectFromKey, subjectKey, subjectName, subjectsOf } from '../overridesModel';

const HOUR = 3_600_000;
/** 2026-11-03 12:00 UTC: noon of the night, in UTC so the words are fixed. */
const NOON = Date.UTC(2026, 10, 3, 12);
const NIGHT = { start_ms: NOON, end_ms: NOON + 24 * HOUR };

const goal = (index: number, overrides: Partial<DirectorScheduleGoal> = {}): DirectorScheduleGoal => ({
  goal_id: `g${index}`, plan_id: 'p1', plan_name: 'Heart Nebula', target_guid: `t${index}`, exposureplan_guid: `e${index}`,
  project_id: 7, target_id: 70 + index, exposure_plan_id: 700 + index, target_name: `IC 1805 r1c${index}`, filter: 'Ha', ...overrides,
});
const goals = [goal(1), goal(2), goal(3, { plan_id: 'p2', plan_name: 'Soul Nebula', project_id: 8, target_name: 'IC 1848' })];

const block = (index: number, start: number, end: number): ScheduleBlock => ({
  target: { project_id: goals[index - 1].project_id, project_name: goals[index - 1].plan_name, target_id: 70 + index, target_name: goals[index - 1].target_name },
  start_ms: start, end_ms: end, slew: true,
  runs: [{ exposure_plan_id: 700 + index, filter: 'Ha', exposure_seconds: 300, frames: 10, dithers: 0, start_ms: start, end_ms: end }],
  decision: { kind: 'ranked', candidates: [] },
});

describe('override subjects', () => {
  it('lists each plan, target and exposure plan once, by name, and round-trips a select value', () => {
    const subjects = subjectsOf(goals);
    expect(subjects.plans.map(p => p.name)).toEqual(['Heart Nebula', 'Soul Nebula']);
    expect(subjects.targets.map(t => t.name)).toEqual(['IC 1805 r1c1', 'IC 1805 r1c2', 'IC 1848']);
    expect(subjects.exposurePlans.map(e => e.name)).toEqual(['IC 1805 r1c1 · Ha', 'IC 1805 r1c2 · Ha', 'IC 1848 · Ha']);
    for (const option of [...subjects.plans, ...subjects.targets, ...subjects.exposurePlans]) {
      expect(subjectFromKey(subjectKey(option.subject))).toEqual(option.subject);
    }
    expect(subjectFromKey('moon:1')).toBeNull();
    expect(subjectName({ kind: 'target', target_guid: 'gone' }, goals)).toBe('a target the rig has no work for');
  });
});

describe('describeOverride', () => {
  it('puts each kind in words, on the given clock', () => {
    const window = { start_ms: NOON + 9 * HOUR, end_ms: NOON + 11 * HOUR };
    const zone = 'UTC';
    const at = (hours: number) => formatClock(NOON + hours * HOUR, zone);
    expect(describeOverride({ kind: 'pin', subject: { kind: 'target', target_guid: 't1' }, window }, goals, zone)).toBe(`Only IC 1805 r1c1 from ${at(9)} to ${at(11)}`);
    expect(describeOverride({ kind: 'exclude', subject: { kind: 'plan', plan_id: 'p2' } }, goals, zone)).toBe('Leave out Soul Nebula all night');
    expect(describeOverride({ kind: 'exclude', subject: { kind: 'exposure_plan', exposureplan_guid: 'e2' }, window }, goals, zone)).toBe(`Leave out IC 1805 r1c2 · Ha from ${at(9)} to ${at(11)}`);
    expect(describeOverride({ kind: 'hold' }, goals, zone)).toBe('Hold the rig all night');
    expect(describeOverride({ kind: 'hold', from_ms: NOON + 14 * HOUR }, goals, zone)).toBe(`Hold the rig from ${at(14)}`);
    expect(describeOverride({ kind: 'order', plans: ['p2', 'p1'] }, goals, zone)).toBe('Plans in this order: Soul Nebula, Heart Nebula');
    // The site's clock, not the browser's: 21:00 UTC is 13:00 in Los Angeles.
    expect(describeOverride({ kind: 'hold', from_ms: NOON + 9 * HOUR }, goals, 'America/Los_Angeles')).toBe(`Hold the rig from ${formatClock(NOON + HOUR, 'UTC')}`);
  });
});

describe('overrideBands', () => {
  it('draws pins, windowed exclusions and holds, and lists a whole-night exclusion only', () => {
    const bands = overrideBands([
      { kind: 'pin', subject: { kind: 'target', target_guid: 't1' }, window: { start_ms: NOON + 9 * HOUR, end_ms: NOON + 10 * HOUR } },
      { kind: 'exclude', subject: { kind: 'plan', plan_id: 'p2' } },
      { kind: 'exclude', subject: { kind: 'plan', plan_id: 'p2' }, window: { start_ms: NOON + 10 * HOUR, end_ms: NOON + 11 * HOUR } },
      { kind: 'hold', from_ms: NOON + 15 * HOUR },
    ], goals, NIGHT, 'UTC');
    expect(bands.map(b => [b.kind, b.start_ms - NOON, b.end_ms - NOON])).toEqual([
      ['pin', 9 * HOUR, 10 * HOUR],
      ['exclude', 10 * HOUR, 11 * HOUR],
      ['hold', 15 * HOUR, 24 * HOUR],
    ]);
  });
});

describe('idlePins', () => {
  const scheduled = (blocks: ScheduleBlock[]): ScheduledNight => ({ date: '2026-11-03', ...NIGHT, blocks, gaps: [], frames: 0, exposure_hours: 0, note: null });
  const window = { start_ms: NOON + 9 * HOUR, end_ms: NOON + 11 * HOUR };

  it('flags a pin whose subject has no frames in its window, and no other', () => {
    const pinned = { kind: 'pin' as const, subject: { kind: 'target' as const, target_guid: 't2' }, window };
    expect(idlePins([pinned], scheduled([block(1, window.start_ms, window.end_ms)]), goals)).toEqual([window]);
    expect(idlePins([pinned], scheduled([block(2, window.start_ms + HOUR, window.end_ms + HOUR)]), goals)).toEqual([]);
    // A subject the rig has no work for is not pinned at all.
    const gone = { ...pinned, subject: { kind: 'target' as const, target_guid: 'gone' } };
    expect(idlePins([gone], scheduled([]), goals)).toEqual([]);
  });
});

describe('moveBlock', () => {
  const moved = block(1, NOON + 8 * HOUR, NOON + 10 * HOUR);
  const target = { kind: 'target' as const, target_guid: 't1' };

  it('pins the target to the new stretch and leaves it out of the part the move uncovers', () => {
    expect(moveBlock(moved, HOUR, goals, NIGHT)).toEqual({ subject: target, entries: [
      { kind: 'pin', subject: target, window: { start_ms: NOON + 9 * HOUR, end_ms: NOON + 11 * HOUR } },
      { kind: 'exclude', subject: target, window: { start_ms: NOON + 8 * HOUR, end_ms: NOON + 9 * HOUR } },
    ] });
    expect(moveBlock(moved, -HOUR, goals, NIGHT)?.entries[1]).toEqual(
      { kind: 'exclude', subject: target, window: { start_ms: NOON + 9 * HOUR, end_ms: NOON + 10 * HOUR } },
    );
  });

  it('measures a block moved again from where it first stood, so nudges do not pile up', () => {
    // The block now runs where the last move pinned it, a quarter hour on.
    const nudged = block(1, NOON + 8 * HOUR + HOUR / 4, NOON + 10 * HOUR + HOUR / 4);
    const origin = { start_ms: NOON + 8 * HOUR, end_ms: NOON + 10 * HOUR };
    expect(moveBlock(nudged, HOUR / 4, goals, NIGHT, origin)?.entries).toEqual([
      { kind: 'pin', subject: target, window: { start_ms: NOON + 8.5 * HOUR, end_ms: NOON + 10.5 * HOUR } },
      { kind: 'exclude', subject: target, window: { start_ms: NOON + 8 * HOUR, end_ms: NOON + 8.5 * HOUR } },
    ]);
    // Back over the start: the target is left out of nothing.
    expect(moveBlock(nudged, -HOUR / 4, goals, NIGHT, origin)?.entries).toEqual([
      { kind: 'pin', subject: target, window: origin },
    ]);
  });

  it('refuses a move out of the night, or of a block whose target cannot be told', () => {
    expect(moveBlock(moved, 15 * HOUR, goals, NIGHT)).toBeNull();
    expect(moveBlock({ ...moved, target: { ...moved.target, target_id: 99 } }, HOUR, goals, NIGHT)).toBeNull();
    expect(moveBlock(moved, 0, goals, NIGHT)).toBeNull();
    // Rows the database did not give: every goal reads as target 0.
    const unread = goals.map(g => ({ ...g, project_id: 0, target_id: 0 }));
    expect(moveBlock({ ...moved, target: { ...moved.target, project_id: 0, target_id: 0 } }, HOUR, unread, NIGHT)).toBeNull();
  });
});
