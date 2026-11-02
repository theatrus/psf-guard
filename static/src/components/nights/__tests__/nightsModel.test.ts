import { describe, expect, it } from 'vitest';
import { availableShares, clockOnNight, completesOn, datesBetween, isDate, loadWords, markGlyph, planColours, planRows, plansOnNight, windowFor } from '../nightsModel';
import { night, rig } from './fixtures';

const HOUR = 3_600_000;

describe('night plan helpers', () => {
  it('puts a clock time on the evening or the morning after', () => {
    const evening = clockOnNight('2026-11-03', '22:30')!;
    const morning = clockOnNight('2026-11-03', '01:15')!;
    expect(new Date(evening).getDate()).toBe(3);
    expect(new Date(morning).getDate()).toBe(4);
    expect(morning - evening).toBe(2 * HOUR + 45 * 60_000);
    expect(clockOnNight('2026-11-03', '25:00')).toBeNull();
    expect(isDate('2026-11-03')).toBe(true);
    expect(isDate('2026-02-30')).toBe(false);
    expect(isDate(null)).toBe(false);
  });

  it('builds a limited window from dusk, to dawn, or between two times', () => {
    const tonight = night(0);
    expect(windowFor(tonight, '', '')).toEqual({ start_ms: tonight.dusk_ms, end_ms: tonight.dawn_ms });
    const late = windowFor(tonight, '23:00', '')!;
    expect(late.start_ms).toBe(clockOnNight(tonight.date, '23:00'));
    expect(late.end_ms).toBe(tonight.dawn_ms);
    // Clipped to the night's span, which runs from the site's mean noon:
    // here half an hour before the clock's.
    const early = { ...tonight, noon_ms: tonight.noon_ms - HOUR / 2 };
    expect(windowFor(early, '', '11:50')!.end_ms).toBe(early.noon_ms + 24 * HOUR);
    // Nothing when the times run backwards.
    expect(windowFor(tonight, '', '13:00')).toBeNull();
    expect(windowFor(tonight, '03:00', '22:00')).toBeNull();
  });

  it('draws the clear part of a night between dusk and dawn', () => {
    expect(availableShares(night(0))).toEqual([{ from: 0, to: 1 }]);
    const limited = night(0, { availability: { state: 'limited', windows: [{ start_ms: night(0).dusk_ms! + 5 * HOUR, end_ms: night(0).dawn_ms! + HOUR }], marks: [] } });
    expect(availableShares(limited)).toEqual([{ from: 0.5, to: 1 }]);
    expect(availableShares(night(0, { availability: { state: 'unavailable', windows: [], marks: [] } }))).toEqual([]);
    expect(markGlyph(limited)).toBe('◐');
    expect(markGlyph(night(0))).toBe('');
    expect(markGlyph(night(0, { availability: { state: 'available', windows: [], marks: [{ scope: 'site', state: 'available', windows: [], note: '' }] } }))).toBe('✓');
  });

  it('says how loaded a night is and which plans can use it', () => {
    expect(loadWords(night(0))).toBe('5.0 h idle');
    expect(loadWords(night(0, { load: { demand_hours: 12, available_hours: 10, idle_hours: 0, overbooked_hours: 2 } }))).toBe('+2.0 h over');
    expect(loadWords(night(0, { load: { demand_hours: 0, available_hours: 10, idle_hours: 10, overbooked_hours: 0 } }))).toBe('idle');
    const owner = rig();
    // Up six hours, one of them Moon-blocked: five usable, all owed.
    expect(plansOnNight(owner, owner.nights[0])).toEqual([{ project_id: 7, name: 'Heart Nebula', hours: 5 }]);
    expect(plansOnNight(owner, owner.nights[1])).toEqual([]);
    expect(planColours([owner, rig({ plans: [{ ...owner.plans[0], name: 'Andromeda' }] })])).toEqual(new Map([['Andromeda', 0], ['Heart Nebula', 1]]));
    expect(datesBetween(owner, owner.nights[2].date, owner.nights[0].date)).toEqual(owner.nights.map(n => n.date));
  });

  it('lists the targets a night applies to, planned first in the schedule\'s order, and folds the rest with a reason', () => {
    const base = rig();
    const plan = (project_id: number, name: string, target_id: number, target: string, meridian_window_minutes = 0) => ({
      ...base.plans[0], project_id, name, limits: { ...base.plans[0].limits, meridian_window_minutes },
      targets: [{ ...base.plans[0].targets[0], target_id, name: target }],
    });
    const progress = (project_id: number, target_id: number, planned: number[], completes_on: number | null) => ({
      project_id, target_id, exposure_plan_id: project_id, filter: 'L', desired: 10, accepted_before: 0, acquired_before: 0, planned, completes_on, complete_before: false,
    });
    const nights = [night(0), night(1), night(2)];
    const block = (project_id: number, project_name: string, target_id: number, start_ms: number) => ({
      target: { project_id, project_name, target_id, target_name: '' }, start_ms, end_ms: start_ms + 3_600_000, slew: true, runs: [],
      decision: { kind: 'only' as const, candidates: [] },
    });
    const noon = nights[1].noon_ms;
    const schedule = {
      runtime: 'target_scheduler' as const,
      nights: nights.map((n, i) => ({ date: n.date, start_ms: n.dusk_ms!, end_ms: n.dawn_ms!, frames: 0, exposure_hours: 0, note: null, gaps: [],
        blocks: i === 1 ? [block(8, 'Andromeda', 80, noon + 9 * HOUR), block(11, 'Pleiades', 110, noon + 8 * HOUR)] : [] })),
      progress: [
        progress(7, 70, [0, 0, 0], null),
        progress(8, 80, [0, 12, 4], 2),
        progress(10, 100, [6, 0, 0], 0),
        progress(11, 110, [0, 3, 0], 1),
      ],
      assumptions: { target_scheduler_version: '5', fidelity: 'estimate' as const, overheads: null, acceptance: 'plan_rate' as const, utc_offset_minutes: 0 },
    };
    const owner = rig({
      plans: [base.plans[0], plan(8, 'Andromeda', 80, 'M 31'), plan(9, 'Orion', 90, 'M 42', 60), plan(10, 'Soul Nebula', 100, 'IC 1848'), plan(11, 'Pleiades', 110, 'M 45')],
      schedule,
    });
    const hours = (project_id: number, target_id: number, up: number, clear: number, blocked: number) =>
      ({ project_id, target_id, hours_up: up, hours_up_available: clear, hours_moon_blocked: blocked, max_altitude_degrees: 70, transit_ms: null });
    const second = night(1, {
      targets: [
        // The Heart: up and clear, but not planned.
        hours(7, 70, 6, 6, 1),
        // Andromeda: planned on this, the second night.
        hours(8, 80, 5, 5, 0),
        // Orion: not up in its meridian window in the dark.
        hours(9, 90, 0, 0, 0),
        // The Soul: up, but done on the first night.
        hours(10, 100, 6, 6, 0),
        // The Pleiades: planned in twilight, with no usable dark hours.
        hours(11, 110, 0, 0, 0),
      ],
    });
    const { applies, out } = planRows(owner, second, schedule);
    expect(applies.map(row => [row.plan.name, row.status, row.planned, row.usable])).toEqual([
      ['Pleiades', 'planned', 3, 0],
      ['Andromeda', 'planned', 12, 5],
      ['Heart Nebula', 'usable', 0, 5],
    ]);
    expect(out.map(row => [row.plan.name, row.status, row.done_on])).toEqual([
      ['Orion', 'outside_window', null],
      ['Soul Nebula', 'done', nights[0].date],
    ]);
    // Without a schedule, nothing is planned or done: what is usable applies.
    const unscheduled = planRows({ ...owner, schedule: null }, second, null);
    expect(unscheduled.applies.map(row => [row.plan.name, row.status])).toEqual([
      ['Soul Nebula', 'usable'], ['Andromeda', 'usable'], ['Heart Nebula', 'usable'],
    ]);
    // A night marked out leaves everything that would be up as not clear;
    // one the Moon blocks entirely says so; one with no darkness says that.
    const marked = night(1, { targets: [hours(7, 70, 6, 0, 0)] });
    expect(planRows(rig(), marked, null).out.map(row => row.status)).toEqual(['not_clear']);
    const moonlit = night(0, { targets: [hours(7, 70, 6, 6, 6)] });
    expect(planRows(rig(), moonlit, null).out.map(row => row.status)).toEqual(['moon']);
    const bright = night(0, { dark_hours: 0, dusk_ms: null, dawn_ms: null, targets: [hours(7, 70, 0, 0, 0)] });
    expect(planRows(rig(), bright, null).out.map(row => row.status)).toEqual(['no_darkness']);
  });

  it('says on which scheduled night a target\'s plans are all done', () => {
    const entry = (completes_on: number | null, complete_before = false) => ({
      project_id: 7, target_id: 70, exposure_plan_id: 1, filter: 'L', desired: 1, accepted_before: 0, acquired_before: 0, planned: [], completes_on, complete_before,
    });
    expect(completesOn([])).toBeNull();
    expect(completesOn([entry(1), entry(3)])).toBe(3);
    expect(completesOn([entry(1), entry(null)])).toBeNull();
    expect(completesOn([entry(null, true)])).toBe(-1);
  });
});
