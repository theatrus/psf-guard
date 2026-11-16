import { fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DirectorVisibleSky } from '../../../api/directorTypes';
import { defaultView, projectedPoint, type LonLat, type PathScale, type SkyView } from '../../../utils/skyProjection';
import { formatNightDate } from '../../nights/nightsModel';
import { formatNight } from '../skyModel';
import SkyMap from '../SkyMap';
import {
  darknessNote,
  formatMinutes,
  nightCount,
  standing,
  visibleSideMarks,
  visibleSkyError,
  visibleSkyLegend,
  visibleSkyShares,
  visibleSkySummary,
} from '../visibleSky';

const AT: PathScale = { cx: 500, cy: 270, scale: 236 };
const flat: SkyView = { frame: 'equatorial', mode: 'aitoff', centerLon: 0, centerLat: 0 };

/** A ring at one declination, run with the north on its left: right
 *  ascension rising. */
const ring = (dec: number): LonLat[] => Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, dec]);

const counts = () => btoa(String.fromCharCode(...new Uint8Array(180 * 91)));
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
  grid: { degrees: 2, columns: 180, rows: 91 },
  counts: counts(),
  every_night: { lines: [ring(-12) as Array<[number, number]>], sky_fraction: 0.41, declinations: [-12.4, 90] },
  any_night: { lines: [ring(-30) as Array<[number, number]>], sky_fraction: 0.62, declinations: [-30.6, 90] },
  ...over,
});

describe('the legend', () => {
  it('gives the rig, the minimum time, the nights and the horizon used', () => {
    const [first, last] = [formatNightDate('2026-09-24'), formatNightDate('2026-10-24')];
    expect(visibleSkyLegend(view(), 'C925')).toBe(`C925: in view 30 min or more · 31 nights, ${first} to ${last} · the site's custom horizon, at least 25°`);
    const year = view({ span: 'year', every_nights: 7, nights: nights('2026-09-24', 53, 7), minimum_minutes: 90, horizon: 'flat', horizon_from: 'none' });
    expect(visibleSkyLegend(year, 'C925')).toBe(`C925: in view 1 h 30 min or more · one night a week for a year, ${formatNight('2026-09-24')} to ${formatNight('2027-09-23')} · flat horizon at 25°`);
    expect(formatMinutes(120)).toBe('2 h');
  });

  it('puts the shares and the declination limits in words, for the map too', () => {
    expect(visibleSkyShares(view())).toBe('In view every night: 41% of the sky, declination −12° to +90°. On at least one night: 62% of the sky, declination −31° to +90°.');
    const never = view({ every_night: { lines: [], sky_fraction: 0, declinations: null } });
    expect(visibleSkyShares(never)).toMatch(/^In view every night: none of the sky\. On at least one night: 62%/);
    expect(visibleSkyShares(view({ any_night: null }))).toBe('In view: 41% of the sky, declination −12° to +90°.');
    expect(visibleSkySummary(view(), 'C925')).toMatch(/^Map of the sky C925: in view 30 min or more · 31 nights.*In view every night: 41% of the sky/);
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

describe('marks on the visible side', () => {
  const ends = (path: string) => [...path.matchAll(/M([\d.-]+),([\d.-]+)L([\d.-]+),([\d.-]+)/g)].map((m) => m.slice(1).map(Number));

  it('point north of a line that runs with rising right ascension, in both map shapes', () => {
    for (const sky of [flat, { ...flat, mode: 'globe' as const, centerLat: 25 }]) {
      const marks = ends(visibleSideMarks(ring(-30), sky, AT, 6, 7));
      expect(marks.length).toBeGreaterThan(10);
      for (const [x1, y1, x2, y2] of marks) {
        expect(Math.hypot(x2 - x1, y2 - y1)).toBeCloseTo(7, 0);
        // North is up on the screen: the mark ends above where it starts.
        expect(y2).toBeLessThan(y1);
      }
    }
  });

  it('keep their direction and length at the deepest zoom', () => {
    const length = 7 / 60;
    const slanted: LonLat[] = Array.from({ length: 41 }, (_, i) => [i * 0.5, -10 + i * 0.3]);
    const marks = ends(visibleSideMarks(slanted, flat, AT, 1, length));
    expect(marks.length).toBeGreaterThan(5);
    const angles = marks.map(([x1, y1, x2, y2]) => (Math.atan2(y2 - y1, x2 - x1) * 180) / Math.PI);
    for (const [x1, y1, x2, y2] of marks) expect(Math.hypot(x2 - x1, y2 - y1)).toBeCloseTo(length, 3);
    for (let i = 1; i < angles.length; i += 1) expect(Math.abs(angles[i] - angles[i - 1])).toBeLessThan(1);
    for (const angle of angles) expect(Math.abs(angle / 45 - Math.round(angle / 45))).toBeGreaterThan(0.05);
  });

  it('point south when the line runs the other way, and start on the line', () => {
    const reversed = [...ring(40)].reverse();
    const marks = ends(visibleSideMarks(reversed, flat, AT, 6, 7));
    expect(marks.length).toBeGreaterThan(10);
    for (const [, y1, , y2] of marks) expect(y2).toBeGreaterThan(y1);
    const onLine = reversed.map(([ra, dec]) => projectedPoint(ra, dec, flat, AT));
    for (const [x1, y1] of marks) expect(onLine.some((p) => Math.abs(p.x - x1) < 0.1 && Math.abs(p.y - y1) < 0.1)).toBe(true);
    expect(visibleSideMarks([[0, 0], [1, 0]], flat, AT, 6, 7)).toBe('');
  });
});

describe('the map', () => {
  afterEach(() => vi.restoreAllMocks());
  const map = (props: Partial<Parameters<typeof SkyMap>[0]> = {}) => render(<SkyMap frame="equatorial" mode="aitoff" showBackdrop={false} showStacks={false} {...props} />).container;

  it('draws with no targets at all, under the name it is given', () => {
    const drawn = map({ label: 'Map of the sky C925' });
    expect(drawn.querySelector('.sky-map')?.getAttribute('aria-label')).toBe('Map of the sky C925');
    expect(drawn.querySelectorAll('.sky-target')).toHaveLength(0);
    expect(drawn.querySelector('.sky-visible')).toBeNull();
    expect(drawn.querySelector('.sky-marks')).toBeNull();
    // Without a name it is the coverage map.
    expect(map().querySelector('.sky-map')?.getAttribute('aria-label')).toBe('Sky coverage map in equatorial coordinates with 0 targets');
  });

  it('draws a solid line for every night and a dashed one for any, with marks', () => {
    const drawn = map({ visible: { every: [ring(-20)], any: [ring(-35)] }, mode: 'globe' });
    expect(drawn.querySelector('.sky-visible')?.getAttribute('data-nights')).toBe('range');
    const every = drawn.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d') ?? '';
    const any = drawn.querySelector('.sky-visible-line.sky-visible-any')?.getAttribute('d') ?? '';
    expect(every).toMatch(/^M/);
    expect(any).toMatch(/^M/);
    expect(any).not.toBe(every);
    expect(drawn.querySelector('.sky-visible-marks.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    expect(drawn.querySelector('.sky-visible')?.getAttribute('pointer-events')).toBe('none');
  });

  it('holds a place for the shading under the lines', () => {
    const shade = { counts: new Uint8Array(180 * 91).fill(3), nights: 31, columns: 180, rows: 91 };
    const drawn = map({ shade });
    const group = drawn.querySelector('.sky-visible-shade');
    expect(group?.getAttribute('data-nights')).toBe('31');
    expect(group?.getAttribute('pointer-events')).toBe('none');
    expect(map().querySelector('.sky-visible-shade')).toBeNull();
  });

  it('marks and labels a target and a picked place where they are', () => {
    const drawn = map({
      marks: [
        { id: 'target', ra: 10.68, dec: 41.27, label: 'M31', kind: 'target' },
        { id: 'pick', ra: 83.8, dec: -5.4, label: '5h 35m, −5° 24′', kind: 'pick' },
      ],
    });
    const target = drawn.querySelector('.sky-mark-target');
    expect(target?.getAttribute('data-mark')).toBe('M31');
    expect(target?.querySelector('text')?.textContent).toBe('M31');
    expect(drawn.querySelector('.sky-mark-pick text')?.textContent).toBe('5h 35m, −5° 24′');
    // The target's diamond sits on its place.
    const at = projectedPoint(10.68, 41.27, defaultView('equatorial', 'aitoff'), AT);
    expect(target?.querySelector('path')?.getAttribute('d')).toContain(`M${at.x} ${at.y - 7}`);
    // Marks never take a click from the map under them.
    expect(drawn.querySelector('.sky-marks')?.getAttribute('pointer-events')).toBe('none');
  });

  it('gives the place under a click on open sky, and nothing for a click off it', () => {
    const onPick = vi.fn();
    const drawn = map({ onPick });
    const svg = drawn.querySelector('.sky-map') as SVGSVGElement;
    expect(svg.getAttribute('data-picks')).toBe('on');
    vi.spyOn(svg, 'getBoundingClientRect').mockReturnValue({ left: 100, top: 50, width: 1000, height: 540, right: 1100, bottom: 590, x: 100, y: 50, toJSON: () => ({}) });
    // The middle of the map is the place it opens turned to.
    const opened = defaultView('equatorial', 'aitoff');
    fireEvent.click(svg, { clientX: 600, clientY: 320 });
    expect(onPick).toHaveBeenCalledTimes(1);
    expect(onPick.mock.calls[0][0]).toBeCloseTo(opened.centerLon, 6);
    expect(onPick.mock.calls[0][1]).toBeCloseTo(0, 6);
    // Where M31 is drawn, M31's place comes back.
    const at = projectedPoint(10.68, 41.27, opened, AT);
    fireEvent.click(svg, { clientX: 100 + at.x, clientY: 50 + at.y });
    expect(onPick.mock.calls[1][0]).toBeCloseTo(10.68, 4);
    expect(onPick.mock.calls[1][1]).toBeCloseTo(41.27, 4);
    // A corner of the picture is not sky.
    fireEvent.click(svg, { clientX: 105, clientY: 55 });
    expect(onPick).toHaveBeenCalledTimes(2);
    // A map that takes no picks does not look as if it did.
    expect(map().querySelector('.sky-map')?.getAttribute('data-picks')).toBeNull();
  });
});

describe('what the Sky view remembers', () => {
  it('lets go of a visible sky choice kept by a build that drew it there', async () => {
    window.sessionStorage.setItem('psf-guard:sky-view:v1', JSON.stringify({ mode: 'globe', zoom: 3, visibleSky: { on: true, rigId: 'rig-1', minimumMinutes: 45 } }));
    // A page opened afresh reads the stored record.
    vi.resetModules();
    const memory = await import('../skyViewMemory');
    expect(memory.recallSkyView()).toEqual({ mode: 'globe', zoom: 3 });
    // The next thing remembered writes the record without it.
    memory.rememberSkyView({ zoom: 4 });
    expect(JSON.parse(window.sessionStorage.getItem('psf-guard:sky-view:v1') ?? '{}')).toEqual({ mode: 'globe', zoom: 4 });
    memory.forgetSkyView();
  });
});
