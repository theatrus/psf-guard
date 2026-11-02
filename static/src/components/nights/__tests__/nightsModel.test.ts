import { describe, expect, it } from 'vitest';
import { availableShares, clockOnNight, completesOn, datesBetween, decidedBy, forecastMark, formatClock, isDate, loadWords, markGlyph, markOrigin, planColours, planRows, plansOnNight, windowFor } from '../nightsModel';
import { NOON, night, rig } from './fixtures';

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
    const hours = [{ start_ms: night(0).dusk_ms! + 5 * HOUR, end_ms: night(0).dawn_ms! + HOUR }];
    const limited = night(0, { availability: { state: 'limited', windows: hours, marks: [{ scope: 'rig', state: 'limited', windows: hours, note: '', source: { kind: 'manual' } }] } });
    expect(availableShares(limited)).toEqual([{ from: 0.5, to: 1 }]);
    expect(availableShares(night(0, { availability: { state: 'unavailable', windows: [], marks: [] } }))).toEqual([]);
    expect(markGlyph(limited)).toBe('◐');
    expect(markGlyph(night(0))).toBe('');
    expect(markGlyph(night(0, { availability: { state: 'available', windows: [], marks: [{ scope: 'site', state: 'available', windows: [], note: '', source: { kind: 'manual' } }] } }))).toBe('✓');
    const fromForecast = night(0, { availability: { state: 'available', windows: [], marks: [{ scope: 'site', state: 'available', windows: [], note: '', source: { kind: 'forecast', provider: 'met_norway', fetched_at_ms: NOON } }] } });
    expect(forecastMark(fromForecast)?.state).toBe('available');
    // The cell shows a cloud for the forecast, not a mark's glyph.
    expect(markGlyph(fromForecast)).toBe('');
    expect(decidedBy(fromForecast)).toBe('forecast');
    // A rig marked available by hand under a cloudy forecast: the forecast
    // decides, and the hand mark's glyph is its own.
    const cloudy = night(0, { availability: { state: 'unavailable', windows: [], marks: [
      { scope: 'site', state: 'unavailable', windows: [], note: '', source: { kind: 'forecast', provider: 'open_meteo', fetched_at_ms: NOON } },
      { scope: 'rig', state: 'available', windows: [], note: '', source: { kind: 'manual' } },
    ] } });
    expect([markGlyph(cloudy), decidedBy(cloudy)]).toEqual(['✓', 'forecast']);
    const both = night(0, { availability: { state: 'unavailable', windows: [], marks: [
      { scope: 'site', state: 'limited', windows: [], note: '', source: { kind: 'forecast', provider: 'open_meteo', fetched_at_ms: NOON } },
      { scope: 'rig', state: 'unavailable', windows: [], note: 'dome', source: { kind: 'manual' } },
    ] } });
    expect([markGlyph(both), decidedBy(both)]).toEqual(['✕', 'both']);
    expect(decidedBy(night(0))).toBe('none');
    expect(forecastMark(night(0))).toBeUndefined();
    expect(markOrigin(forecastMark(fromForecast)!)).toBe(`Forecast (MET Norway, fetched ${formatClock(NOON)})`);
    expect(markOrigin(fromForecast.availability.marks[0])).toMatch(/^Forecast/);
    expect(markOrigin({ scope: 'rig', state: 'unavailable', windows: [], note: '', source: { kind: 'manual' } })).toBe('Rig');
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

describe('clocks in a site time zone', () => {
  it('reads and shows times on the site clock, across a change to standard time', async () => {
    const { clockOnNight, clockValue } = await import('../nightsModel');
    // 1 November 2026: Los Angeles leaves daylight time at 02:00, so the
    // evening and the small hours after it are both eight hours behind UTC.
    expect(clockOnNight('2026-11-01', '21:00', 'America/Los_Angeles')).toBe(Date.UTC(2026, 10, 2, 5, 0));
    expect(clockOnNight('2026-11-01', '01:30', 'America/Los_Angeles')).toBe(Date.UTC(2026, 10, 2, 9, 30));
    // The evening before the change is still seven hours behind.
    expect(clockOnNight('2026-10-31', '21:00', 'America/Los_Angeles')).toBe(Date.UTC(2026, 10, 1, 4, 0));
    expect(clockOnNight('2026-07-01', '22:15', 'Europe/Berlin')).toBe(Date.UTC(2026, 6, 1, 20, 15));
    // An hour after the change: a guess read at the old offset is an hour
    // early, which the second pass corrects.
    expect(clockOnNight('2026-10-31', '03:00', 'America/Los_Angeles')).toBe(Date.UTC(2026, 10, 1, 11, 0));
    expect(clockValue(Date.UTC(2026, 10, 2, 9, 30), 'America/Los_Angeles')).toBe('01:30');
    expect(clockValue(Date.UTC(2026, 6, 1, 20, 15), 'Europe/Berlin')).toBe('22:15');
  });
});

describe('a zone this browser does not know', () => {
  it('falls back to the browser\'s clock instead of failing', async () => {
    const { clockValue, formatClock, usableZone } = await import('../nightsModel');
    expect(usableZone('Mars/Olympus_Mons')).toBeUndefined();
    expect(usableZone('Europe/Berlin')).toBe('Europe/Berlin');
    expect(usableZone(null)).toBeUndefined();
    const at = Date.UTC(2026, 10, 3, 4, 0);
    expect(formatClock(at, 'Mars/Olympus_Mons')).toBe(formatClock(at));
    expect(clockValue(at, 'Mars/Olympus_Mons')).toBe(clockValue(at));
  });
});
