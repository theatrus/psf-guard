import { describe, expect, it } from 'vitest';
import type { DirectorVisibleSky } from '../../../api/directorTypes';
import type { LonLat } from '../../../utils/skyProjection';
import { formatNightDate } from '../../nights/nightsModel';
import { formatNight } from '../skyModel';
import {
  darknessNote,
  formatMinutes,
  nightCount,
  standing,
  visibleSkyError,
  visibleSkyLegend,
  visibleSkyShares,
  visibleSkySummary,
} from '../visibleSky';


/** A ring at one declination, run with the north on its left: right
 *  ascension rising. */
const ring = (dec: number): LonLat[] => Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, dec]);

const counts = () => btoa('\0'.repeat(180 * 91 * 2));
const nights = (first: string, count: number, every: number) =>
  Array.from({ length: count }, (_, i) => ({ date: new Date(Date.parse(`${first}T00:00:00Z`) + i * every * 86_400_000).toISOString().slice(0, 10), dusk_ms: 1, dawn_ms: 2 }));

const view = (over: Partial<DirectorVisibleSky> = {}): DirectorVisibleSky => ({
  rig: { id: 'rig-1', name: 'Rig', revision: 1 },
  planning_site: { id: 'site-1', name: 'Patio', revision: 1 },
  site: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 },
  location_from: 'site',
  horizon: 'custom',
  horizon_from: 'site',
  limits: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, horizon_offset_degrees: 0 },
  minimum_minutes: 30,
  rig_minimum_minutes: 30,
  step_minutes: 5,
  dark_below_degrees: -12,
  nights: nights('2026-09-24', 31, 1),
  span: 'now',
  every_nights: 1,
  nights_counted: 31,
  grid: { degrees: 2, columns: 180, rows: 91 },
  counts: counts(),
  every_night: { lines: [ring(-12) as Array<[number, number]>], sky_fraction: 0.41, declinations: [-12.4, 90] },
  any_night: { lines: [ring(-30) as Array<[number, number]>], sky_fraction: 0.62, declinations: [-30.6, 90] },
  ...over,
});

describe('the legend', () => {
  it('gives the rig, the minimum time, the nights and the horizon used', () => {
    const [first, last] = [formatNightDate('2026-09-24'), formatNightDate('2026-10-24')];
    // Nights asked for without a date start from tonight at the rig's site.
    expect(visibleSkyLegend(view(), 'C925')).toBe(`C925: in view 30 min or more · 31 nights from tonight, ${first}, to ${last} · the site's custom horizon, at least 25°`);
    expect(visibleSkyLegend(view(), 'C925', false)).toBe(`C925: in view 30 min or more · 31 nights ${first} to ${last} · the site's custom horizon, at least 25°`);
    expect(visibleSkyLegend(view({ nights: nights('2026-09-24', 1, 1) }), 'C925')).toContain(`· tonight, ${first} ·`);
    const year = view({ span: 'year', every_nights: 7, nights: nights('2026-09-24', 53, 7), nights_counted: 365, minimum_minutes: 90, horizon: 'flat', horizon_from: 'none' });
    expect(visibleSkyLegend(year, 'C925')).toBe(`C925: in view 1 h 30 min or more · 365 nights from tonight, ${formatNight('2026-09-24')}, to ${formatNight('2027-09-23')}, one a week measured · flat horizon at 25°`);
    expect(formatMinutes(120)).toBe('2 h');
  });

  it('puts the shares and the declination limits in words, for the map too', () => {
    expect(visibleSkyShares(view())).toBe('In view every night: 41% of the sky, declination −12° to +90°. On at least one night: 62% of the sky, declination −31° to +90°.');
    const never = view({ every_night: { lines: [], sky_fraction: 0, declinations: null } });
    expect(visibleSkyShares(never)).toMatch(/^In view every night: none of the sky\. On at least one night: 62%/);
    expect(visibleSkyShares(view({ any_night: null }))).toBe('In view: 41% of the sky, declination −12° to +90°.');
    expect(visibleSkySummary(view(), 'C925')).toMatch(/^Map of the sky C925: in view 30 min or more · 31 nights from tonight.*In view every night: 41% of the sky/);
  });

  it('places a count among the lines', () => {
    expect(standing(31, 31)).toBe('every');
    expect(standing(30, 31)).toBe('some');
    expect(standing(1, 53)).toBe('some');
    expect(standing(0, 53)).toBe('none');
  });

  it('notes nights that never get dark', () => {
    const light = view({ nights: [...nights('2026-06-10', 2, 1), { date: '2026-06-12', dusk_ms: null, dawn_ms: null }] });
    expect(darknessNote(light)).toBe('It does not get dark on 1 of these nights.');
    // A year's note is about the nights measured, one a week.
    expect(darknessNote({ ...light, span: 'year' })).toBe('It does not get dark on 1 of the 3 nights measured.');
    expect(darknessNote(view())).toBeNull();
  });

  it('reads a refusal about the site from the server', () => {
    const refused = { isAxiosError: true, response: { status: 422, data: { error: 'Rig has no location. Choose a Planning site in Rig setup.' } } };
    expect(visibleSkyError(refused)).toEqual({ text: 'Rig has no location. Choose a Planning site in Rig setup.', needsSite: true });
    // As the API client hands it on: the message, with the response as its cause.
    expect(visibleSkyError(new Error(refused.response.data.error, { cause: refused }))).toEqual({ text: 'Rig has no location. Choose a Planning site in Rig setup.', needsSite: true });
    expect(visibleSkyError(new Error('boom'))).toEqual({ text: 'The sky in view could not be worked out.', needsSite: false });
  });

  it('steps the night at local mean noon at the site', () => {
    const noonUtc = Date.parse('2026-09-24T12:00:00Z');
    expect(nightCount(noonUtc + 1, 0)).toBe(nightCount(noonUtc - 1, 0) + 1);
    expect(nightCount(noonUtc + 1, -118.3)).toBe(nightCount(noonUtc - 1, -118.3));
  });
});
