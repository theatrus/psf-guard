import { describe, expect, it } from 'vitest';
import { availableShares, clockOnNight, datesBetween, isDate, loadWords, markGlyph, planColours, plansOnNight, windowFor } from '../nightsModel';
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
});
