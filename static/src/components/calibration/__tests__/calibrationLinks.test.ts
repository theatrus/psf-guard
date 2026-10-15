import { describe, expect, it } from 'vitest';
import { calibrationGridHref, calibrationMasterHref, calibrationPageHref, nightDates } from '../calibrationLinks';

/** Whether the Images grid keeps a light taken at `acquired` (seconds) under
 *  these dates: the same comparison as its date filter. */
function kept(acquired: number, dates: { dateStart: string; dateEnd: string }) {
  const at = new Date(acquired * 1000);
  return at >= new Date(dates.dateStart) && at <= new Date(dates.dateEnd);
}

describe('nightDates', () => {
  it('covers exactly the lights the server counts under that night', () => {
    // Nights split at 19:00 UTC: night 2026-07-01 runs from 2026-07-01
    // 19:00 UTC up to 2026-07-02 19:00 UTC.
    const dates = nightDates('2026-07-01', 19 * 3600)!;
    const start = Date.UTC(2026, 6, 1, 19) / 1000;
    expect(kept(start - 1, dates)).toBe(false);
    expect(kept(start, dates)).toBe(true);
    expect(kept(start + 86_400 - 1, dates)).toBe(true);
    expect(kept(start + 86_400, dates)).toBe(false);
  });

  it('gives no dates for a night that is not a date', () => {
    expect(nightDates('unknown', 43_200)).toBeUndefined();
  });

  it('puts the dates on a grid link, and clears old ones from one without', () => {
    const dates = nightDates('2026-07-01', 12 * 3600)!;
    const withDates = new URLSearchParams(
      calibrationGridHref('?db=a&dateStart=2020-01-01', 'a', 3, 'cannot_calibrate', ['HA'], dates).split('?')[1],
    );
    expect(withDates.get('dateStart')).toBe('2026-07-01T12:00:00.000Z');
    expect(withDates.get('dateEnd')).toBe('2026-07-02T11:59:59.999Z');
    const without = new URLSearchParams(
      calibrationGridHref('?db=a&dateStart=2020-01-01', 'a', 3, 'cannot_calibrate').split('?')[1],
    );
    expect(without.has('dateStart')).toBe(false);
    expect(without.has('dateEnd')).toBe(false);
  });
});

describe('calibrationPageHref', () => {
  const query = (href: string) => new URLSearchParams(href.split('?')[1]);

  it("opens on a project's calibration when it names one, and on the whole library otherwise", () => {
    const scoped = query(calibrationPageHref('?db=a&project=3&calProject=5&calKind=dark', 'a', 3));
    expect(scoped.get('project')).toBe('3');
    expect(scoped.get('calProject')).toBe('3');
    expect(scoped.has('calKind')).toBe(false);
    expect(query(calibrationPageHref('?db=a&project=3&calProject=3', 'a')).has('calProject')).toBe(false);
    // A master is found in the whole library.
    expect(query(calibrationMasterHref('?db=a&project=3', 'a', 'm1')).has('calProject')).toBe(false);
  });
});
