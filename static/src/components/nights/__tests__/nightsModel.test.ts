import { describe, expect, it } from 'vitest';
import { availableShares, clockOnNight, datesBetween, isDate, loadWords, markGlyph, planColours, planRows, plansOnNight, windowFor } from '../nightsModel';
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

  it('lists the targets a night applies to, planned first, and folds the rest with a reason', () => {
    const base = rig();
    const second = { ...base.plans[0], project_id: 8, name: 'Andromeda', targets: [{ ...base.plans[0].targets[0], target_id: 80, name: 'M 31' }] };
    const third = { ...base.plans[0], project_id: 9, name: 'Orion', targets: [{ ...base.plans[0].targets[0], target_id: 90, name: 'M 42' }] };
    const owner = rig({
      plans: [base.plans[0], second, third],
      schedule: {
        runtime: 'target_scheduler',
        nights: [],
        progress: [{ project_id: 8, target_id: 80, exposure_plan_id: 1, filter: 'L', desired: 10, accepted_before: 0, acquired_before: 0, planned: [12, 0, 0], completes_on: null, complete_before: false }],
        assumptions: { target_scheduler_version: '5', fidelity: 'estimate', overheads: null, acceptance: 'plan_rate', utc_offset_minutes: 0 },
      },
    });
    const first = night(0, {
      targets: [
        // The Heart: up and clear, but nothing planned for it.
        { project_id: 7, target_id: 70, hours_up: 6, hours_up_available: 6, hours_moon_blocked: 1, max_altitude_degrees: 70, transit_ms: null },
        // Andromeda: planned.
        { project_id: 8, target_id: 80, hours_up: 5, hours_up_available: 5, hours_moon_blocked: 0, max_altitude_degrees: 80, transit_ms: null },
        // Orion: never above its limits in the dark.
        { project_id: 9, target_id: 90, hours_up: 0, hours_up_available: 0, hours_moon_blocked: 0, max_altitude_degrees: 10, transit_ms: null },
      ],
    });
    const { tonight, other } = planRows(owner, first);
    expect(tonight.map(row => [row.plan.name, row.status, row.planned, row.usable])).toEqual([
      ['Andromeda', 'planned', 12, 5],
      ['Heart Nebula', 'up', 0, 5],
    ]);
    expect(other.map(row => [row.plan.name, row.status])).toEqual([['Orion', 'below_limits']]);
    // A night marked out leaves everything that would be up as not clear;
    // one the Moon blocks entirely says so.
    const marked = night(1, { targets: [{ project_id: 7, target_id: 70, hours_up: 6, hours_up_available: 0, hours_moon_blocked: 0, max_altitude_degrees: 70, transit_ms: null }] });
    expect(planRows(rig(), marked).other.map(row => row.status)).toEqual(['not_clear']);
    const moonlit = night(0, { targets: [{ project_id: 7, target_id: 70, hours_up: 6, hours_up_available: 6, hours_moon_blocked: 6, max_altitude_degrees: 70, transit_ms: null }] });
    expect(planRows(rig(), moonlit).other.map(row => row.status)).toEqual(['moon']);
  });
});
