import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, configure, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useMemo } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DirectorSkyPosition, DirectorVisiblePlacesRequest, DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { formatNightDate } from '../nights/nightsModel';
import { formatNight } from '../sky/skyModel';
import { DEFAULT_STAGE, deprojectOn, fromStage, projectOn, skyAtStage, toStage, viewAt } from '../director/framingModel';
import { SkyInViewLayer, SkyInViewPanel } from '../director/SkyInView';
import { monthMarks, useSkyInView, type SkyInViewRig } from '../director/useSkyInView';
import { COVER_REST_MS, coverSize, stageLines, stagePlace, stageReach } from '../director/skyInViewStage';
import { coverRaster, type SkyCounts } from '../sky/skyShade';

configure({ asyncUtilTimeout: 5000 });

const asked = vi.hoisted(() => ({ calls: [] as DirectorVisibleSkyRequest[], places: [] as DirectorVisiblePlacesRequest[], answer: (request: DirectorVisibleSkyRequest): Promise<unknown> => Promise.resolve(request) }));
vi.mock('../../api/client', () => ({
  apiClient: {
    getDirectorVisibleSky: (request: DirectorVisibleSkyRequest) => { asked.calls.push(request); return asked.answer(request); },
    // Each place counted for itself, by the rule of the counts: every
    // night north of +40°, 12 nights down to −30°, none south of that.
    getDirectorVisiblePlaces: async (request: DirectorVisiblePlacesRequest) => {
      asked.places.push(request);
      const counted = request.span === 'year' ? 365 : 31;
      if (request.span === 'year') return { first_night: request.first_night ?? '2026-09-24', nights_counted: 365, at_least_nights: 7, places: request.places.map((place) => ({ ...place, nights_in_view: yearNights(place.dec_degrees), at_least: yearNights(place.dec_degrees) >= 7 })) };
      return { first_night: request.first_night ?? '2026-09-24', nights_counted: counted, places: request.places.map((place) => ({ ...place, nights_in_view: place.dec_degrees >= 40 ? counted : place.dec_degrees >= -30 ? 12 : 0 })) };
    },
  },
}));
/** A year in the tests: every night north of +40°, 7 nights down to
 *  +10°, 3 down to −30°, none south of that. */
const yearNights = vi.hoisted(() => (dec: number) => (dec >= 40 ? 365 : dec >= 10 ? 7 : dec >= -30 ? 3 : 0));
const openSettings = vi.hoisted(() => vi.fn());
vi.mock('../../utils/settingsIntent', () => ({ openSettings }));

const counts = (nights: number) => {
  let text = '';
  for (let row = 0; row < 91; row += 1) {
    const count = -90 + row * 2 >= 40 ? nights : -90 + row * 2 >= -30 ? 12 : 0;
    text += String.fromCharCode(count & 255, count >> 8).repeat(180);
  }
  return btoa(text);
};
/** A year's measured nights: north of +40° in view on all 53, down to
 *  −30° on the first ten only, never south of that. */
const measuredYear = () => {
  const bytes = new Uint8Array(53 * 2048);
  for (let night = 0; night < 53; night += 1) {
    for (let index = 0; index < 180 * 91; index += 1) {
      const dec = -90 + Math.floor(index / 180) * 2;
      if (dec >= 40 || (dec >= -30 && night < 10)) bytes[night * 2048 + (index >> 3)] |= 1 << (index & 7);
    }
  }
  let text = '';
  for (let i = 0; i < bytes.length; i += 4096) text += String.fromCharCode(...bytes.subarray(i, i + 4096));
  return { in_view: btoa(text), bytes_a_night: 2048, turn_degrees_a_night: 1 };
};
const yearCounts = () => {
  let text = '';
  for (let row = 0; row < 91; row += 1) {
    const count = yearNights(-90 + row * 2);
    text += String.fromCharCode(count & 255, count >> 8).repeat(180);
  }
  return btoa(text);
};
const ring = (dec: number) => Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, dec] as [number, number]);
const nightList = (count: number, every: number, first = '2026-09-24') =>
  Array.from({ length: count }, (_, i) => ({ date: new Date(Date.parse(`${first}T00:00:00Z`) + i * every * 86_400_000).toISOString().slice(0, 10), dusk_ms: Date.now() + 1, dawn_ms: Date.now() + 2 }));
const view = (request: DirectorVisibleSkyRequest, over: Partial<DirectorVisibleSky> = {}): DirectorVisibleSky => {
  const year = request.span === 'year';
  return {
    rig: { id: request.rig, name: 'Rig', revision: 1 },
    planning_site: { id: 'site', name: 'Patio', revision: 1 },
    site: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 },
    location_from: 'site', horizon: 'custom', horizon_from: 'site',
    limits: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, horizon_offset_degrees: 0 },
    minimum_minutes: request.minimum_minutes ?? 30, rig_minimum_minutes: 30, step_minutes: 5, dark_below_degrees: -12,
    nights: nightList(year ? 53 : 31, year ? 7 : 1), span: year ? 'year' : 'now', every_nights: year ? 7 : 1,
    nights_counted: year ? 365 : 31,
    grid: { degrees: 2, columns: 180, rows: 91 }, counts: year ? yearCounts() : counts(31),
    every_night: { lines: [ring(40)], sky_fraction: 0.18, declinations: [39.6, 90] },
    any_night: { lines: [ring(-30)], sky_fraction: 0.75, declinations: [-30.4, 90] },
    ...(year ? { at_least: { nights: 7, lines: [ring(10)], sky_fraction: 0.41, declinations: [9.8, 90] as [number, number] } } : {}),
    ...(request.measured ? { measured: measuredYear() } : {}),
    ...over,
  };
};

const rig: SkyInViewRig = { id: 'rig-1', name: 'C925', slug: 'c925' };
const stage = DEFAULT_STAGE;

/** The overlay as Framing holds it: the layer on a stage looking at
 *  `looking`, and the panel beside it. */
const LOOKING = { ra_degrees: 38.2, dec_degrees: 0 };
function Harness({ target, viewCenter = null, on = true, rigs = rig as SkyInViewRig | null, waiting = null, targetMoving = false, ownFraming = false, turn = 0, fov = 150 }: { target: DirectorSkyPosition | null; viewCenter?: DirectorSkyPosition | null; on?: boolean; rigs?: SkyInViewRig | null; waiting?: string | null; targetMoving?: boolean; ownFraming?: boolean; turn?: number; fov?: number }) {
  const [shown, setShown] = [on, vi.fn()];
  const model = useSkyInView({ rig: rigs, waiting, on: shown, target, targetMoving, ownFraming, viewCenter });
  const stageView = useMemo(() => viewAt(LOOKING, LOOKING, turn), [turn]);
  return (
    <>
      <svg viewBox={`0 0 ${stage.width} ${stage.height}`}>
        {model.layer && <SkyInViewLayer layer={model.layer} view={stageView} viewFov={fov} stage={stage} />}
      </svg>
      <ul>{model.notes.map((note) => <li key={note.text} data-testid="note" title={note.title}>{note.text}</li>)}</ul>
      <p data-testid="stale">{String(model.panel.stale)}</p>
      <SkyInViewPanel model={model} onShow={setShown} />
    </>
  );
}
function show(props: Parameters<typeof Harness>[0]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = (over: Partial<Parameters<typeof Harness>[0]> = {}) => <QueryClientProvider client={client}><Harness {...props} {...over} /></QueryClientProvider>;
  const rendered = render(tree());
  return { ...rendered, client, again: (over: Partial<Parameters<typeof Harness>[0]>) => rendered.rerender(tree(over)) };
}
const status = () => screen.getByTestId('sky-in-view-status').textContent ?? '';
const notes = () => screen.queryAllByTestId('note').map((note) => [note.textContent, note.getAttribute('title')]);

describe('sky in view over the framing stage', () => {
  beforeEach(() => {
    asked.calls.length = 0; asked.places.length = 0; asked.answer = async (request) => view(request); openSettings.mockClear();
    // No canvas under the tests: the layer draws without the cover's picture.
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  });
  afterEach(() => vi.restoreAllMocks());

  it('draws the cover and both lines for the coming month, and says in words where the target stands', async () => {
    const { container } = show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toBe('The target is in view on all of the 31 nights counted.'));
    expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }]);
    // The target itself was counted, once.
    expect(asked.places).toEqual([{ rig: 'rig-1', span: 'now', minimum_minutes: 30, first_night: '2026-09-24', places: [{ ra_degrees: 38.2, dec_degrees: 61.45 }] }]);
    const layer = container.querySelector('.sky-in-view-layer');
    expect(layer?.getAttribute('data-nights')).toBe('31');
    expect(container.querySelector('.sky-in-view-line.sky-in-view-every')?.getAttribute('d')).toMatch(/^M/);
    expect(container.querySelector('.sky-in-view-line.sky-in-view-any')?.getAttribute('d')).toMatch(/^M/);
    expect(container.querySelector('.sky-in-view-marks.sky-in-view-every')?.getAttribute('d')).toMatch(/^M/);
    // A target in view every night needs no note on the stage.
    expect(notes()).toEqual([]);
    // Nothing here is read out as it changes.
    expect(container.querySelector('[role="status"], [aria-live]')).toBeNull();
    // The panel's words: what was counted, and what it comes to.
    expect(container.querySelector('.sky-in-view-body')?.textContent).toContain(`C925: in view 30 min or more · 31 nights from tonight, ${formatNightDate('2026-09-24')}, to ${formatNightDate('2026-10-24')} · the site's custom horizon, at least 25°.`);
    expect(screen.getByTestId('sky-in-view-shares').textContent).toBe('In view every night: 18% of the sky, declination +40° to +90°. On at least one night: 75% of the sky, declination −30° to +90°.');
    expect((screen.getByLabelText('Minimum minutes') as HTMLInputElement).value).toBe('30');
    expect(screen.getByRole('radio', { name: 'Now' }).getAttribute('aria-checked')).toBe('true');
  });

  it('puts a few words on the stage when the target is not in view every night, or never', async () => {
    const { again } = show({ target: { ra_degrees: 38.2, dec_degrees: 0 } });
    await waitFor(() => expect(status()).toBe('The target is in view on 12 of the 31 nights counted.'));
    expect(notes()).toEqual([['Target not in view every night', 'The target is in view on 12 of the 31 nights counted.']]);
    again({ target: { ra_degrees: 38.2, dec_degrees: -60 } });
    await waitFor(() => expect(status()).toBe('The target is never in view on the 31 nights counted.'));
    expect(notes()).toEqual([['Target never in view', 'The target is never in view on the 31 nights counted.']]);
    // The middle of the view gets words of its own once it is somewhere
    // else, and the target keeps its own while the view moves.
    again({ target: { ra_degrees: 38.2, dec_degrees: 61.45 }, viewCenter: { ra_degrees: 38.2, dec_degrees: 61.6 } });
    await waitFor(() => expect(status()).toBe('The target is in view on all of the 31 nights counted.'));
    expect(screen.queryByTestId('sky-in-view-view')).toBeNull();
    again({ target: { ra_degrees: 38.2, dec_degrees: 61.45 }, viewCenter: { ra_degrees: 100, dec_degrees: -50 } });
    await waitFor(() => expect(screen.getByTestId('sky-in-view-view').textContent).toBe('The middle of the view is never in view on the 31 nights counted.'));
    expect(status()).toBe('The target is in view on all of the 31 nights counted.');
    expect(notes()).toEqual([['This sky is never in view', 'The middle of the view is never in view on the 31 nights counted.']]);
  });

  it('counts the year, and a longer minimum once typing stops, with the way back', async () => {
    const { container } = show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(status()).toBe('The target is in view on all of the 365 nights of the year.'));
    // Year alone does not ask for the measured nights.
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'year', priority: 'interactive' });
    // The cover is weighed against the 7 nights that leave sky clear, not
    // against the 365, and the solid line is the server's line round that sky.
    const layer = container.querySelector('.sky-in-view-layer');
    expect(layer?.getAttribute('data-nights')).toBe('7');
    expect(layer?.querySelector('.sky-in-view-line.sky-in-view-every')?.getAttribute('d')).toMatch(/^M/);
    const body = container.querySelector('.sky-in-view-body')?.textContent ?? '';
    expect(body).toContain('Year counts every night of the year. Clear sky is in view on at least 7 of them, in total and not in a row. One night a week is measured and the nights between are worked out from the measured nights. The solid line is within about 1° of a line measured on every night.');
    expect(body).toContain('Covered sky is in view on fewer than 7 nights. Hatched sky is never in view.');
    // The scale and the solid line's name follow.
    expect(body).toContain('No night7 nights or moreEdge of the sky in view on 7 nights or moreEdge of the sky in view on any night');
    expect(screen.getByTestId('sky-in-view-shares').textContent).toBe('In view on 7 nights or more: 41% of the sky, declination +10° to +90°. On at least one night: 75% of the sky, declination −30° to +90°.');
    expect(notes()).toEqual([]);
    fireEvent.click(screen.getByRole('radio', { name: 'Now' }));
    const field = screen.getByLabelText('Minimum minutes') as HTMLInputElement;
    fireEvent.change(field, { target: { value: '9' } });
    fireEvent.change(field, { target: { value: '90' } });
    fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'now', minimum_minutes: 90, priority: 'interactive' }));
    expect(asked.calls.some((call) => call.minimum_minutes === 9)).toBe(false);
    fireEvent.click(await screen.findByRole('button', { name: "Use rig's 30 min" }));
    await waitFor(() => expect(field.value).toBe('30'));
  });

  it('in Year notes a target only when it falls short of a week of nights, and keeps the exact count in words', async () => {
    const { again, container } = show({ target: { ra_degrees: 38.2, dec_degrees: 20 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    // In the month, 12 of 31 nights is short of every night.
    expect(notes()).toEqual([['Target not in view every night', 'The target is in view on 12 of the 31 nights counted.']]);
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    // Seven nights of the year: clear, and no note.
    await waitFor(() => expect(status()).toBe('The target is in view on 7 of the 365 nights of the year.'));
    expect(notes()).toEqual([]);
    // Three nights: short of a week.
    again({ target: { ra_degrees: 38.2, dec_degrees: 0 } });
    await waitFor(() => expect(status()).toBe('The target is in view on 3 of the 365 nights of the year.'));
    expect(notes()).toEqual([['Target in view under a week a year', 'The target is in view on 3 of the 365 nights of the year.']]);
    // None.
    again({ target: { ra_degrees: 38.2, dec_degrees: -60 } });
    await waitFor(() => expect(status()).toBe('The target is never in view on the 365 nights of the year.'));
    expect(notes()).toEqual([['Target never in view', 'The target is never in view on the 365 nights of the year.']]);
    // The middle of the view, somewhere else, by the same rule.
    again({ target: { ra_degrees: 38.2, dec_degrees: 20 }, viewCenter: { ra_degrees: 100, dec_degrees: -10 } });
    await waitFor(() => expect(screen.getByTestId('sky-in-view-view').textContent).toBe('The middle of the view is in view on 3 of the 365 nights of the year.'));
    await waitFor(() => expect(notes()).toEqual([['This sky is in view under a week a year', 'The middle of the view is in view on 3 of the 365 nights of the year.']]));
    // Back in the month the scale and the line are the every-night ones.
    fireEvent.click(screen.getByRole('radio', { name: 'Now' }));
    await waitFor(() => expect(container.querySelector('.sky-in-view-layer')?.getAttribute('data-nights')).toBe('31'));
    expect(container.querySelector('.sky-in-view-body')?.textContent).toContain('No nightEvery nightEdge of the sky in view every night');
  });

  it('shows a month of the year at a time under a slider, asking once', async () => {
    const { container } = show({ target: { ra_degrees: 38.2, dec_degrees: 0 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    const slider = (await screen.findByRole('slider', { name: 'Time of year' })) as HTMLInputElement;
    await waitFor(() => expect(slider.disabled).toBe(false));
    // Season asks for the year with its measured nights.
    expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }, { rig: 'rig-1', span: 'year', measured: true, priority: 'interactive' }]);
    expect([slider.min, slider.max, slider.step, slider.value]).toEqual(['0', '48', '1', '0']);
    expect(slider.getAttribute('aria-valuetext')).toBe(`31 nights from ${formatNight('2026-09-24')}`);
    expect(status()).toBe('The target is in view on all of the 31 nights counted.');
    const legend = container.querySelector('.sky-in-view-text')?.textContent;
    expect(legend).toBe(`C925: in view 30 min or more · 31 nights at a time from tonight, ${formatNight('2026-09-24')}, to ${formatNight('2027-09-23')}, one a week measured · the site's custom horizon, at least 25°.`);
    expect(screen.getByTestId('sky-in-view-shares').textContent).toBe(`${formatNight('2026-09-24')} to ${formatNight('2026-10-24')}. In view every night: 76% of the sky, declination −31° to +90°. On at least one night: 76% of the sky, declination −31° to +90°.`);
    const before = container.querySelector('.sky-in-view-line.sky-in-view-every')?.getAttribute('d');

    // Seven weeks on, the southern band is in view on the first 15 nights only.
    fireEvent.change(slider, { target: { value: '7' } });
    expect(slider.getAttribute('aria-valuetext')).toBe(`31 nights from ${formatNight('2026-11-12')}`);
    expect(status()).toBe('The target is in view on 15 of the 31 nights counted.');
    expect(notes()).toEqual([['Target not in view every night', 'The target is in view on 15 of the 31 nights counted.']]);
    expect(screen.getByTestId('sky-in-view-shares').textContent).toContain(`${formatNight('2026-11-12')} to ${formatNight('2026-12-12')}. In view every night: 19% of the sky, declination +39° to +90°.`);
    expect(container.querySelector('.sky-in-view-line.sky-in-view-every')?.getAttribute('d')).not.toBe(before);
    // What stays the same along the slider does not change, so the line
    // above it never grows or shrinks.
    expect(container.querySelector('.sky-in-view-text')?.textContent).toBe(legend);
    // Page Up and Page Down move about a month.
    fireEvent.keyDown(slider, { key: 'PageUp' });
    expect(slider.value).toBe('11');
    expect(status()).toBe('The target is never in view on the 31 nights counted.');
    fireEvent.keyDown(slider, { key: 'PageDown' });
    fireEvent.keyDown(slider, { key: 'PageDown' });
    fireEvent.keyDown(slider, { key: 'PageDown' });
    expect(slider.value).toBe('0');
    fireEvent.change(slider, { target: { value: '48' } });
    expect(screen.getByTestId('sky-in-view-shares').textContent).toContain(`${formatNight('2027-08-24')} to ${formatNight('2027-09-23')}.`);
    // None of it asked the server for anything, about the picture or a place.
    expect(asked.calls).toHaveLength(2);
    expect(asked.places.every((request) => request.span === 'now')).toBe(true);
    // Year reads Season's answer: going between them asks for nothing.
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(container.querySelector('.sky-in-view-layer')?.getAttribute('data-nights')).toBe('7'));
    expect(screen.queryByRole('slider')).toBeNull();
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    expect((screen.getByRole('slider', { name: 'Time of year' }) as HTMLInputElement).value).toBe('48');
    expect(asked.calls).toHaveLength(2);
  });

  it('draws the cover small while anything moves and whole once it rests: the slider, the turn of the sky, the zoom', async () => {
    const { again } = show({ target: { ra_degrees: 38.2, dec_degrees: 0 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    const slider = (await screen.findByRole('slider', { name: 'Time of year' })) as HTMLInputElement;
    await waitFor(() => expect(slider.disabled).toBe(false));
    const sizes: number[] = [];
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(function (this: HTMLCanvasElement) {
      sizes.push(this.width);
      return null;
    });
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'requestAnimationFrame', 'cancelAnimationFrame'] });
    const rest = () => {
      act(() => { vi.advanceTimersByTime(COVER_REST_MS + 5); });
      act(() => { vi.advanceTimersByTime(40); });
    };
    try {
      rest();
      const moves: Array<Array<() => void>> = [
        [5, 6, 7].map((week) => () => { fireEvent.change(slider, { target: { value: String(week) } }); }),
        [20, 21, 22].map((turn) => () => again({ turn })),
        [90, 80, 70].map((fov) => () => again({ turn: 22, fov })),
      ];
      for (const [kind, steps] of moves.entries()) {
        sizes.length = 0;
        // Three moves in a row, a frame apart: each is drawn small.
        for (const step of steps) {
          step();
          act(() => { vi.advanceTimersByTime(20); });
        }
        expect(sizes.length).toBeGreaterThanOrEqual(3);
        expect(sizes, `move ${kind}`).toEqual(sizes.map(() => 128));
        rest();
        expect(sizes.at(-1)).toBe(384);
        // And once at rest it is not drawn again.
        const drawn = sizes.length;
        act(() => { vi.advanceTimersByTime(1000); });
        expect(sizes.length).toBe(drawn);
      }
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the last words, dimmed, while the target moves, and counts it once it rests', async () => {
    const { again } = show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toBe('The target is in view on all of the 31 nights counted.'));
    expect(screen.getByTestId('stale').textContent).toBe('false');
    const asks = asked.places.length;
    // Dragged: Framing still hands over the place it rested at.
    again({ target: { ra_degrees: 38.2, dec_degrees: 61.45 }, targetMoving: true });
    expect(status()).toBe('The target is in view on all of the 31 nights counted.');
    expect(screen.getByTestId('stale').textContent).toBe('true');
    expect(screen.getByTestId('sky-in-view-status').getAttribute('data-stale')).toBe('true');
    expect(asked.places.length).toBe(asks);
    // At rest somewhere new: the old words stay until the new count is here.
    again({ target: { ra_degrees: 38.2, dec_degrees: -60 }, targetMoving: false });
    expect(status()).toBe('The target is in view on all of the 31 nights counted.');
    expect(screen.getByTestId('stale').textContent).toBe('true');
    await waitFor(() => expect(status()).toBe('The target is never in view on the 31 nights counted.'));
    expect(screen.getByTestId('stale').textContent).toBe('false');
    expect(asked.places.length).toBe(asks + 1);
  });

  it("speaks of a rig's own framing when that is what it shoots, and of waiting when the rig is not known yet", async () => {
    const { again } = show({ target: { ra_degrees: 38.2, dec_degrees: -60 }, ownFraming: true });
    await waitFor(() => expect(status()).toBe("C925's own framing is never in view on the 31 nights counted."));
    expect(notes()).toEqual([["C925's framing never in view", "C925's own framing is never in view on the 31 nights counted."]]);
    again({ rigs: null, waiting: 'Loading the rigs…' });
    expect(status()).toBe('Loading the rigs…');
    again({ rigs: null, waiting: null });
    expect(status()).toBe('This plan has no rig yet.');
  });

  it('asks as a page for the picture drawn unasked, and as a person for one chosen here', async () => {
    show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }]);
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'year', priority: 'interactive' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    await waitFor(() => expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'year', measured: true, priority: 'interactive' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Now' }));
    const field = screen.getByLabelText('Minimum minutes') as HTMLInputElement;
    fireEvent.change(field, { target: { value: '90' } });
    fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'now', minimum_minutes: 90, priority: 'interactive' }));
    // A count for a place is never a person's request.
    expect(asked.places.every((request) => !('priority' in request))).toBe(true);
  });

  it('shows the remedy, and no slider, when the year for Season cannot be fetched', async () => {
    asked.answer = async (request) => { if (request.measured) throw new Error('boom'); return view(request); };
    show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    await waitFor(() => expect(status()).toBe('The sky in view could not be worked out.'));
    expect(screen.queryByRole('slider')).toBeNull();
    expect(screen.queryByText('Working out the year…')).toBeNull();
  });

  it('says so when a year cannot be read, and leaves Year to use', async () => {
    asked.answer = async (request) => view(request, request.measured ? { measured: { in_view: 'not base64!', bytes_a_night: 2048, turn_degrees_a_night: 1 } } : {});
    const { container } = show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    await waitFor(() => expect(status()).toBe("The year's nights could not be read, so Season has nothing to show. Reload the page, or choose Year."));
    expect(screen.queryByRole('slider')).toBeNull();
    expect(container.querySelector('.sky-in-view-layer')).toBeNull();
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(status()).toBe('The target is in view on all of the 365 nights of the year.'));
  });

  it('explains a rig with no site, and asks for nothing while hidden or without a rig', async () => {
    asked.answer = async () => { throw new Error('C925 has no location. Choose a Planning site in Rig setup.', { cause: { isAxiosError: true, response: { status: 422, data: { error: 'C925 has no location. Choose a Planning site in Rig setup.' } } } }); };
    const { container, again } = show({ target: { ra_degrees: 38.2, dec_degrees: 61.45 } });
    await waitFor(() => expect(status()).toBe('C925 has no location. Choose a Planning site in Rig setup.'));
    expect(container.querySelector('.sky-in-view-layer')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Open Rig setup' }));
    expect(openSettings).toHaveBeenCalledWith('rigs', { databaseId: 'c925' });
    const before = asked.calls.length;
    again({ on: false });
    expect(status()).toBe('Hidden');
    expect((screen.getByLabelText('Show on the sky') as HTMLInputElement).checked).toBe(false);
    expect(screen.queryByRole('radio', { name: 'Now' })).toBeNull();
    again({ rigs: null });
    expect(status()).toBe('This plan has no rig yet.');
    await act(async () => { await Promise.resolve(); });
    expect(asked.calls.length).toBe(before);
  });
});

describe('the months along the slider', () => {
  it('leaves out a month that begins at either end, so no name overhangs', () => {
    // From 2 December the last place begins on 1 November: at the very end.
    const late = monthMarks('2026-12-02', 334);
    expect(late.every((mark) => mark.share >= 0.04 && mark.share <= 0.96)).toBe(true);
    expect(late).toHaveLength(10);
    // From 31 January the first month begins a night in.
    const early = monthMarks('2027-01-31', 334);
    expect(early.every((mark) => mark.share >= 0.04 && mark.share <= 0.96)).toBe(true);
    expect(early[0].share).toBeCloseTo(29 / 334, 6);
    // From mid month all eleven fit.
    expect(monthMarks('2026-11-16', 334)).toHaveLength(11);
    expect(monthMarks(undefined, 334)).toEqual([]);
  });
});

describe('the stage', () => {
  const looking = { ra_degrees: 120, dec_degrees: 35 };

  it('finds the sky under a point the way Framing draws it, and back', () => {
    for (const rotation of [0, 37]) {
      const stageView = viewAt({ ra_degrees: 118, dec_degrees: 33 }, looking, rotation);
      for (const fov of [0.5, 20, 150]) {
        const place = stagePlace(stageView, fov, stage);
        for (const [x, y] of [[0, 0], [stage.width / 2, stage.height / 2], [stage.width, stage.height], [200, 600]]) {
          const [ra, dec] = place(x, y);
          const back = projectOn(stageView, { ra_degrees: ra, dec_degrees: dec });
          expect(back).not.toBeNull();
          const [bx, by] = toStage(back!, fov, stage);
          expect(Math.abs(bx - x)).toBeLessThan(1e-6);
          expect(Math.abs(by - y)).toBeLessThan(1e-6);
        }
        // The same as Framing's own reading of a point.
        expect(skyAtStage(stageView, fov, 300, 200, stage)).toEqual(deprojectOn(stageView, fromStage(300, 200, fov, stage)));
      }
    }
    // At the widest field the corners are still on the near side of the sky.
    const wide = stagePlace(viewAt(looking, looking), 150, stage);
    const rad = Math.PI / 180;
    for (const [x, y] of [[0, 0], [stage.width, 0], [0, stage.height], [stage.width, stage.height]]) {
      const [ra, dec] = wide(x, y);
      const cos = Math.sin(dec * rad) * Math.sin(35 * rad) + Math.cos(dec * rad) * Math.cos(35 * rad) * Math.cos((ra - 120) * rad);
      expect(Math.acos(cos) / rad).toBeLessThan(90);
    }
  });

  it('draws a line across the view with marks on the side of the sky in view', () => {
    const north = viewAt({ ra_degrees: 120, dec_degrees: 0 }, { ra_degrees: 120, dec_degrees: 0 });
    // A line along the equator, run toward rising right ascension: the
    // sky in view is north of it, up on a stage with north up.
    const equator = [Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, 0] as [number, number])];
    const drawn = stageLines(equator, north, 40, stage);
    const marks = [...drawn.marks.matchAll(/M([\d.-]+) ([\d.-]+)l([\d.-]+) ([\d.-]+)/g)].map((m) => m.slice(1).map(Number));
    expect(marks.length).toBeGreaterThan(10);
    for (const [, y, dx, dy] of marks) {
      expect(y).toBeCloseTo(stage.height / 2, 0);
      expect(Math.abs(dx)).toBeLessThan(0.2);
      expect(dy).toBeLessThan(-6);
    }
    // Run the other way, the marks point down.
    const back = stageLines([[...equator[0]].reverse()], north, 40, stage);
    expect([...back.marks.matchAll(/l([\d.-]+) ([\d.-]+)/g)].every((m) => Number(m[2]) > 6)).toBe(true);
    // With the sky turned a quarter of a turn, the line stands upright and
    // the marks still point into the same sky.
    const turned = stageLines(equator, viewAt({ ra_degrees: 120, dec_degrees: 0 }, { ra_degrees: 120, dec_degrees: 0 }, 90), 40, stage);
    const side = [...turned.marks.matchAll(/l([\d.-]+) ([\d.-]+)/g)].map((m) => [Number(m[1]), Number(m[2])]);
    expect(side.length).toBeGreaterThan(5);
    expect(new Set(side.map(([dx]) => Math.sign(dx))).size).toBe(1);
    expect(side.every(([dx, dy]) => Math.abs(dx) > 6 && Math.abs(dy) < 0.2)).toBe(true);
    // Nothing is drawn far past the stage, however far the line runs.
    const numbers = [...drawn.line.matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)].map((m) => [Number(m[1]), Number(m[2])]);
    expect(numbers.every(([x, y]) => x >= -41 && x <= stage.width + 41 && y >= -41 && y <= stage.height + 41)).toBe(true);
  });

  it('keeps a line that crosses a view narrower than the gap between its points', () => {
    // Half a degree across, and points two degrees apart either side of it.
    const tight = viewAt({ ra_degrees: 121, dec_degrees: 10 }, { ra_degrees: 121, dec_degrees: 10 });
    const drawn = stageLines([[[120, 10], [122, 10]]], tight, 0.5, stage);
    // East is to the left: the line runs from the right edge to the left.
    expect(drawn.line).toMatch(/^M1064\.0 [\d.]+L-40\.0 [\d.]+$/);
    // A line nowhere near the view draws nothing.
    expect(stageLines([[[120, 40], [122, 40]]], tight, 0.5, stage)).toEqual({ line: '', marks: '' });
    // Nor does one on the far side of the sky.
    expect(stageLines([[[300, -10], [302, -10]]], tight, 150, stage).line).toBe('');
  });

  it('reaches as far as its corners, so an upright stage keeps the lines its cover is drawn to', () => {
    // A wide stage reaches 79° at the widest zoom; an upright one, 95°.
    expect(stageReach(150, stage)).toBeCloseTo(78.6, 0);
    const upright = { width: 768, height: 1024 };
    expect(stageReach(150, upright)).toBeGreaterThan(94);
    expect(stageReach(1, upright)).toBeLessThan(1);
    // Two places near the upright stage's bottom corner, further from the
    // middle than a wide stage reaches: the stretch between them is drawn.
    const middle = { ra_degrees: 120, dec_degrees: 60 };
    const corner = stagePlace(viewAt(middle, middle), 150, upright);
    const low: Array<[number, number]> = [[...corner(40, 1000)] as [number, number], [...corner(120, 1010)] as [number, number]];
    const rad = Math.PI / 180;
    for (const [ra, dec] of low) {
      const cos = Math.sin(dec * rad) * Math.sin(60 * rad) + Math.cos(dec * rad) * Math.cos(60 * rad) * Math.cos((ra - 120) * rad);
      expect(Math.acos(cos) / rad).toBeGreaterThan(86);
    }
    const onUpright = stageLines([low], viewAt(middle, middle), 150, upright);
    expect(onUpright.line).toMatch(/^M40\.0 1000\.0L120\.0 1010\.0$/);
    // The wide stage does not reach them.
    expect(stageLines([low], viewAt(middle, middle), 150, stage).line).toBe('');
    // The far side of the sky is still left out on any stage.
    expect(stageLines([[[298, -60], [300, -60], [302, -60]]], viewAt(middle, middle), 150, upright).line).toBe('');
  });

  it('draws the cover\'s edge where the line is, at the bottom of a stage, at both sizes of raster', () => {
    // In view north of +21°, never south of +19°: the edge is at +20°,
    // and so is the line. A stage 1024 by 700, 20° across, looking where
    // the line runs near its bottom.
    const counts = new Uint16Array(180 * 91);
    for (let row = 0; row < 91; row += 1) counts.fill(-90 + row * 2 >= 22 ? 30 : -90 + row * 2 <= 18 ? 0 : 15, row * 180, (row + 1) * 180);
    const sky: SkyCounts = { counts, nights: 30, columns: 180, rows: 91 };
    const wide = { width: 1024, height: 700 };
    const looking = { ra_degrees: 120, dec_degrees: 26 };
    const stageView = viewAt(looking, looking);
    const [, lineY] = toStage(projectOn(stageView, { ra_degrees: 120, dec_degrees: 20 })!, 20, wide);
    expect(lineY).toBeGreaterThan(wide.height * 0.9);
    for (const across of [384, 128]) {
      const [width, height] = coverSize(wide, across);
      const pixels = coverRaster(sky, wide.width, wide.height, width, height, stagePlace(stageView, 20, wide));
      const column = Math.floor(width / 2);
      const down = Array.from({ length: height }, (_, py) => pixels[(py * width + column) * 4 + 3]);
      // Where the cover passes half, between the middles of two pixels.
      const below = down.findIndex((value) => value >= 127.5);
      expect(below).toBeGreaterThan(0);
      const part = (127.5 - down[below - 1]) / (down[below] - down[below - 1]);
      const edgeY = (below - 0.5 + part) * (wide.height / height);
      expect(Math.abs(edgeY - lineY), `${across} across`).toBeLessThan(across === 384 ? 0.6 : 1.5);
    }
  });
});
