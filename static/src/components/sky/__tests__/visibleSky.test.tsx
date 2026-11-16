import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DirectorVisibleSky } from '../../../api/directorTypes';
import { projectedPoint, type LonLat, type PathScale, type SkyView } from '../../../utils/skyProjection';
import SkyMap from '../SkyMap';
import SkyVisible from '../SkyVisible';
import { forgetSkyView, recallSkyView, rememberSkyView } from '../skyViewMemory';
import {
  NO_VISIBLE_SKY,
  addDays,
  darknessNote,
  formatMinutes,
  nightsBetween,
  rangeProblem,
  visibleSideMarks,
  visibleSkyError,
  visibleSkyLegend,
  visibleSkyRequest,
  visibleSkyShares,
  type VisibleSkyChoice,
} from '../visibleSky';

const openSettings = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/settingsIntent', () => ({ openSettings }));

const AT: PathScale = { cx: 500, cy: 270, scale: 236 };
const flat: SkyView = { frame: 'equatorial', mode: 'aitoff', centerLon: 0, centerLat: 0 };

/** A ring at one declination, run with the north on its left: right
 *  ascension rising. */
const ring = (dec: number): LonLat[] => Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, dec]);

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
  nights: [{ date: '2026-09-24', dusk_ms: 1, dawn_ms: 2 }],
  every_night: { lines: [ring(-30) as Array<[number, number]>], sky_fraction: 0.62 },
  any_night: null,
  ...over,
});

const choice = (over: Partial<VisibleSkyChoice> = {}): VisibleSkyChoice => ({ ...NO_VISIBLE_SKY, on: true, rigId: 'rig-1', ...over });
const rigs = [
  { id: 'rig-1', name: 'C925', slug: 'c925' },
  { id: 'rig-2', name: 'RC51', slug: 'rc51' },
];
const NOW = Date.parse('2026-09-24T20:00:00');

describe('the Visible sky request', () => {
  it('asks for nothing while off or without a rig, and for tonight by default', () => {
    expect(visibleSkyRequest(NO_VISIBLE_SKY)).toBeNull();
    expect(visibleSkyRequest(choice({ rigId: null }))).toBeNull();
    expect(visibleSkyRequest(choice())).toEqual({ rig: 'rig-1' });
    expect(visibleSkyRequest(choice({ minimumMinutes: 90 }))).toEqual({ rig: 'rig-1', minimum_minutes: 90 });
  });

  it('counts a range of dates with both ends, and refuses one the server would', () => {
    expect(nightsBetween('2026-09-24', '2026-10-07')).toBe(14);
    expect(nightsBetween('2026-09-24', '2026-09-24')).toBe(1);
    expect(nightsBetween('2026-09-24', '2026-09-23')).toBeNull();
    expect(addDays('2026-12-25', 13)).toBe('2027-01-07');
    const range = choice({ when: 'dates', from: '2026-09-24', to: '2026-10-07' });
    expect(visibleSkyRequest(range)).toEqual({ rig: 'rig-1', first_night: '2026-09-24', nights: 14 });
    expect(rangeProblem(range)).toBeNull();
    const long = choice({ when: 'dates', from: '2026-09-24', to: '2026-10-25' });
    expect(visibleSkyRequest(long)).toBeNull();
    expect(rangeProblem(long)).toBe('Choose 31 nights or fewer.');
    expect(rangeProblem(choice({ when: 'dates', from: '2026-09-24', to: null }))).toBe('Choose a first and a last night.');
  });
});

describe('the Visible sky legend', () => {
  it('gives the rig, the minimum time, the night and the horizon used', () => {
    expect(visibleSkyLegend(view(), 'C925', 1)).toBe("C925: in view 30 min or more · tonight (24 Sep) · the site's custom horizon, at least 25°");
    // After dawn the night under way has ended.
    expect(visibleSkyLegend(view(), 'C925', 2)).toContain('· last night (24 Sep) ·');
    expect(visibleSkyLegend(view(), 'C925', null)).toContain('· 24 Sep ·');
    const range = view({
      minimum_minutes: 90,
      horizon: 'flat',
      horizon_from: 'none',
      nights: [
        { date: '2026-09-24', dusk_ms: 1, dawn_ms: 2 },
        { date: '2026-10-07', dusk_ms: null, dawn_ms: null },
      ],
      any_night: { lines: [], sky_fraction: 0.7 },
    });
    expect(visibleSkyLegend(range, 'C925', null)).toBe('C925: in view 1 h 30 min or more · 24 Sep to 7 Oct · flat horizon at 25°');
    expect(visibleSkyShares(range)).toBe('Solid line: every night, 62% of the sky. Dashed line: at least one night, 70%.');
    expect(visibleSkyShares(view())).toBe('The line encloses 62% of the sky.');
    expect(darknessNote(range)).toBe('It does not get dark on 1 of these nights.');
    expect(darknessNote(view())).toBeNull();
    expect(formatMinutes(120)).toBe('2 h');
  });

  it('reads a refusal about the site from the server', () => {
    const refused = { isAxiosError: true, response: { status: 422, data: { error: 'Rig has no location. Choose a Planning site in Rig setup.' } } };
    expect(visibleSkyError(refused)).toEqual({ text: 'Rig has no location. Choose a Planning site in Rig setup.', needsSite: true });
    // As the API client hands it on: the message, with the response as its cause.
    expect(visibleSkyError(new Error(refused.response.data.error, { cause: refused }))).toEqual({ text: 'Rig has no location. Choose a Planning site in Rig setup.', needsSite: true });
    expect(visibleSkyError(new Error('boom'))).toEqual({ text: 'The visible sky could not be worked out.', needsSite: false });
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

describe('the lines on the map', () => {
  const map = (visible: Parameters<typeof SkyMap>[0]['visible'], mode: 'aitoff' | 'globe' = 'aitoff') =>
    render(<SkyMap targets={[]} frame="equatorial" mode={mode} showBackdrop={false} showStacks={false} onOpen={() => {}} visible={visible} />).container;

  it('draws one solid line with marks for one night, and nothing when off', () => {
    expect(map(null).querySelector('.sky-visible')).toBeNull();
    const drawn = map({ every: [ring(-30)], any: null });
    expect(drawn.querySelector('.sky-visible')?.getAttribute('data-nights')).toBe('one');
    expect(drawn.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).toMatch(/^M[\d.,-]+L/);
    expect(drawn.querySelector('.sky-visible-marks.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    expect(drawn.querySelector('.sky-visible-any')).toBeNull();
  });

  it('draws a second, dashed line for a range, in the globe too', () => {
    const drawn = map({ every: [ring(-20)], any: [ring(-35)] }, 'globe');
    expect(drawn.querySelector('.sky-visible')?.getAttribute('data-nights')).toBe('range');
    const every = drawn.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d') ?? '';
    const any = drawn.querySelector('.sky-visible-line.sky-visible-any')?.getAttribute('d') ?? '';
    expect(every).toMatch(/^M/);
    expect(any).toMatch(/^M/);
    expect(any).not.toBe(every);
    // The lines never take a click from the fields under them.
    expect(drawn.querySelector('.sky-visible')?.getAttribute('pointer-events')).toBe('none');
  });
});

describe('the Visible sky controls', () => {
  beforeEach(() => openSettings.mockClear());
  const show = (props: Partial<Parameters<typeof SkyVisible>[0]> = {}) => {
    const onChange = vi.fn();
    render(<SkyVisible rigs={rigs} choice={choice()} onChange={onChange} view={view({ nights: [{ date: '2026-09-24', dusk_ms: NOW + 1, dawn_ms: NOW + 2 }] })} loading={false} error={null} nowMs={NOW} {...props} />);
    return onChange;
  };

  it('starts from the rig and its minimum time, for tonight', () => {
    show();
    expect(screen.getByRole('group', { name: 'Visible sky' })).toBeTruthy();
    expect((screen.getByLabelText('Rig') as HTMLSelectElement).value).toBe('rig-1');
    expect((screen.getByLabelText('Minimum minutes') as HTMLInputElement).value).toBe('30');
    expect(screen.getByRole('radio', { name: 'Tonight' }).getAttribute('aria-checked')).toBe('true');
    expect(screen.queryByLabelText('First night')).toBeNull();
    expect(screen.queryByRole('button', { name: /Use rig's/ })).toBeNull();
    expect(screen.getByRole('status').textContent).toContain("C925: in view 30 min or more · tonight (24 Sep) · the site's custom horizon, at least 25°. The line encloses 62% of the sky. Marks point into the visible sky.");
  });

  it('takes a typed minimum, offers the way back, and starts a new rig from its own value', () => {
    const onChange = show({ choice: choice({ minimumMinutes: 90 }), view: view({ minimum_minutes: 90 }) });
    fireEvent.change(screen.getByLabelText('Minimum minutes'), { target: { value: '120' } });
    expect(onChange).toHaveBeenLastCalledWith(choice({ minimumMinutes: 120 }));
    // Not a time yet: nothing is asked for.
    onChange.mockClear();
    fireEvent.change(screen.getByLabelText('Minimum minutes'), { target: { value: '' } });
    fireEvent.change(screen.getByLabelText('Minimum minutes'), { target: { value: '0' } });
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: "Use rig's 30 min" }));
    expect(onChange).toHaveBeenLastCalledWith(choice({ minimumMinutes: null }));
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-2' } });
    expect(onChange).toHaveBeenLastCalledWith(choice({ rigId: 'rig-2', minimumMinutes: null }));
  });

  it('opens a two-week range of dates from today and takes other dates', () => {
    const onChange = show();
    fireEvent.click(screen.getByRole('radio', { name: 'Dates' }));
    expect(onChange).toHaveBeenLastCalledWith(choice({ when: 'dates', from: '2026-09-24', to: '2026-10-07' }));
  });

  it('shows the dates of a range and why a range cannot be drawn', () => {
    const onChange = show({ choice: choice({ when: 'dates', from: '2026-09-24', to: '2026-10-30' }) });
    expect((screen.getByLabelText('First night') as HTMLInputElement).value).toBe('2026-09-24');
    expect((screen.getByLabelText('Last night') as HTMLInputElement).max).toBe('2026-10-24');
    expect(screen.getByRole('status').textContent).toBe('Choose 31 nights or fewer.');
    fireEvent.change(screen.getByLabelText('Last night'), { target: { value: '2026-10-01' } });
    expect(onChange).toHaveBeenLastCalledWith(choice({ when: 'dates', from: '2026-09-24', to: '2026-10-01' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Tonight' }));
    expect(onChange).toHaveBeenLastCalledWith(choice({ when: 'tonight', from: '2026-09-24', to: '2026-10-30' }));
  });

  it('shows that it is working before the first lines arrive', () => {
    show({ view: undefined, loading: true });
    expect(screen.getByRole('status').textContent).toBe('Working out the visible sky…');
  });

  it('explains a rig with no site and opens its Rig setup', () => {
    const error = { isAxiosError: true, response: { status: 422, data: { error: 'C925 has no location. Choose a Planning site in Rig setup.' } } };
    show({ view: undefined, error });
    expect(screen.getByRole('status').textContent).toContain('C925 has no location. Choose a Planning site in Rig setup.');
    fireEvent.click(screen.getByRole('button', { name: 'Open Rig setup' }));
    expect(openSettings).toHaveBeenCalledWith('rigs', { databaseId: 'c925' });
  });
});

describe('remembering the choice', () => {
  it('keeps the Visible sky choice with the rest of the view', () => {
    forgetSkyView();
    rememberSkyView({ visibleSky: choice({ minimumMinutes: 45, when: 'dates', from: '2026-09-24', to: '2026-10-07' }) });
    expect(recallSkyView().visibleSky).toEqual(choice({ minimumMinutes: 45, when: 'dates', from: '2026-09-24', to: '2026-10-07' }));
    expect(JSON.parse(window.sessionStorage.getItem('psf-guard:sky-view:v1') ?? '{}').visibleSky.rigId).toBe('rig-1');
    forgetSkyView();
  });
});
