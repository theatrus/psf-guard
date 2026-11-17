import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DirectorVisiblePlacesRequest, DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { SKY_INPUTS_MUTATION } from '../director/skyInputs';
import { formatNightDate } from '../nights/nightsModel';
import { formatNight } from '../sky/skyModel';
import PlanSky, { MINIMUM_SETTLE_MS, SEASON_SETTLE_MS } from '../director/PlanSky';

// The map is a heavy thing to draw; a busy machine needs more than a second.
configure({ asyncUtilTimeout: 5000 });

const asked = vi.hoisted(() => ({ calls: [] as DirectorVisibleSkyRequest[], places: [] as DirectorVisiblePlacesRequest[], placesFail: false, answer: (request: DirectorVisibleSkyRequest): Promise<unknown> => Promise.reject(new Error(`unset ${request.rig}`)) }));
const openSettings = vi.hoisted(() => vi.fn());
vi.mock('../../utils/settingsIntent', () => ({ openSettings }));
vi.mock('../../api/client', () => ({
  apiClient: {
    getDirectorVisibleSky: (request: DirectorVisibleSkyRequest) => { asked.calls.push(request); return asked.answer(request); },
    // Each place counted for itself, by the same rule as the counts: every
    // night north of +40°, 12 nights down to −30°, none south of that.
    getDirectorVisiblePlaces: async (request: DirectorVisiblePlacesRequest) => {
      asked.places.push(request);
      if (asked.placesFail) throw new Error('boom');
      const counted = request.span === 'year' ? 365 : 31;
      return { first_night: request.first_night ?? '2026-09-24', nights_counted: counted, places: request.places.map((place) => ({ ...place, nights_in_view: place.dec_degrees >= 40 ? counted : place.dec_degrees >= -30 ? 12 : 0 })) };
    },
    searchDirectorNames: async (query: string) => ({ query, local: { available: true, items: query.toLowerCase().startsWith('m 3') ? [{ name: 'M 31', common_name: 'Andromeda Galaxy', kind: 'galaxy', ra_degrees: 10.68, dec_degrees: 41.27, matched: 'M 31', source: 'Seiza object catalog' }] : [] }, online: null, online_state: 'skipped', online_cached: false }),
    resolveDirectorName: async (name: string) => { throw new Error(`No catalog knows "${name}"`); },
  },
}));

/** Counts as the server packs them: in view every night north of +40°,
 *  on 12 nights down to −30°, never south of that. */
const counts = (nights: number) => {
  let text = '';
  for (let row = 0; row < 91; row += 1) {
    const count = -90 + row * 2 >= 40 ? nights : -90 + row * 2 >= -30 ? 12 : 0;
    // Two bytes a node, low byte first.
    text += String.fromCharCode(count & 255, count >> 8).repeat(180);
  }
  return btoa(text);
};
/** A year's measured nights as the server packs them: north of +40° in
 *  view on all 53, down to −30° on the first ten only, never south of that. */
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
const ring = (dec: number) => Array.from({ length: 181 }, (_, i) => [(i * 2) % 360, dec] as [number, number]);
const nightList = (count: number, every: number) =>
  Array.from({ length: count }, (_, i) => ({ date: new Date(Date.parse('2026-09-24T00:00:00Z') + i * every * 86_400_000).toISOString().slice(0, 10), dusk_ms: Date.now() + 1, dawn_ms: Date.now() + 2 }));
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
    grid: { degrees: 2, columns: 180, rows: 91 }, counts: counts(year ? 365 : 31),
    every_night: { lines: [ring(40)], sky_fraction: 0.18, declinations: [39.6, 90] },
    any_night: { lines: [ring(-30)], sky_fraction: 0.75, declinations: [-30.4, 90] },
    ...(year ? { measured: measuredYear() } : {}),
    ...over,
  };
};
const rigs = [
  { id: 'rig-1', name: 'C925', slug: 'c925' },
  { id: 'rig-2', name: 'RC51', slug: 'rc51' },
];
const target = { name: 'IC 1805', center: { ra_degrees: 38.2, dec_degrees: 61.45 } };

function show(props: Partial<Parameters<typeof PlanSky>[0]> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const handlers = { onFrame: vi.fn(), onRigs: vi.fn() };
  const tree = (over: Partial<Parameters<typeof PlanSky>[0]> = {}) => <QueryClientProvider client={client}><PlanSky rigs={rigs} target={target} frameBlocked={null} shown {...handlers} {...props} {...over} /></QueryClientProvider>;
  const rendered = render(tree());
  return { ...handlers, ...rendered, client, again: (over: Partial<Parameters<typeof PlanSky>[0]>) => rendered.rerender(tree(over)) };
}
/** A save, as a form elsewhere on the page makes one. */
const save = (client: QueryClient, mutationKey: readonly string[]) => act(async () => { await client.getMutationCache().build(client, { mutationKey, mutationFn: async () => 'saved' }).execute(undefined); });
const legend = () => screen.getByRole('status').textContent ?? '';

describe("the plan's Sky tab", () => {
  beforeEach(() => {
    asked.calls.length = 0; asked.places.length = 0; asked.placesFail = false; asked.answer = async (request) => view(request); openSettings.mockClear();
    // No canvas under the tests: the map draws without the shading's picture.
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  });
  afterEach(() => vi.restoreAllMocks());

  it('asks for nothing until the tab is opened', async () => {
    show({ shown: false });
    await act(async () => { await Promise.resolve(); });
    expect(asked.calls).toEqual([]);
    // Nor is the map laid out behind another tab.
    expect(screen.getByRole('region', { name: 'Sky in view' })).toBeEmptyDOMElement();
  });

  it("opens on the plan's first rig for the coming month, at the rig's minimum time", async () => {
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('C925: in view 30 min or more'));
    expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }]);
    expect(legend()).toBe(`C925: in view 30 min or more · 31 nights from tonight, ${formatNightDate('2026-09-24')}, to ${formatNightDate('2026-10-24')} · the site's custom horizon, at least 25°. In view every night: 18% of the sky, declination +40° to +90°. On at least one night: 75% of the sky, declination −30° to +90°.`);
    expect((screen.getByLabelText('Rig') as HTMLSelectElement).value).toBe('rig-1');
    expect((screen.getByLabelText('Minimum minutes') as HTMLInputElement).value).toBe('30');
    expect(screen.getByRole('radio', { name: 'Now' }).getAttribute('aria-checked')).toBe('true');
    expect(screen.getByRole('radio', { name: 'Flat' }).getAttribute('aria-checked')).toBe('true');
    // The map states the numbers for readers who cannot see it, and holds
    // the shading and both lines.
    const map = container.querySelector('.sky-map');
    expect(map?.getAttribute('aria-label')).toMatch(/^Map of the sky C925: in view 30 min or more.*In view every night: 18% of the sky, declination \+40° to \+90°\. On at least one night: 75%/);
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('31');
    expect(container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    expect(container.querySelector('.sky-visible-line.sky-visible-any')?.getAttribute('d')).toMatch(/^M/);
    // The scale and the lines are explained in words.
    const key = screen.getByLabelText('How to read the map');
    expect(key.textContent).toBe('No nightEvery nightEdge of the sky in view every nightEdge of the sky in view on any nightMarks point into the sky in view.Covered sky is out of view on some nights. Hatched sky is never in view.');
    // The plan's target is marked, with the nights it is in view.
    expect(container.querySelector('.sky-mark-target')?.getAttribute('data-mark')).toBe('IC 1805');
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('Target: IC 1805');
    // The card asks about the target itself, for the picture's own nights.
    await waitFor(() => expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 31 of the 31 nights counted. Inside the every-night line.'));
    expect(asked.places).toEqual([{ rig: 'rig-1', span: 'now', minimum_minutes: 30, first_night: '2026-09-24', places: [{ ra_degrees: 38.2, dec_degrees: 61.45 }] }]);
    // A fetch marks the tab busy; it adds no words.
    expect(screen.getByRole('region', { name: 'Sky in view' }).getAttribute('aria-busy')).toBe('false');
  });

  it('counts every night of a year, and takes another rig, map shape and frame', async () => {
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(legend()).toContain('365 nights from tonight, '));
    expect(legend()).toContain(', one a week measured');
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'year' });
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('365');
    await waitFor(() => expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 365 of the 365 nights of the year. Inside the every-night line.'));
    // The key explains which nights are measured and how close the line is.
    expect(screen.getByLabelText('How to read the map').textContent).toContain('Year counts every night. One night a week is measured and the nights between are worked out from the measured nights. The solid line is within about 2° of a line measured on every night.');
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-2' } });
    await waitFor(() => expect(legend()).toContain('RC51: in view'));
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-2', span: 'year' });
    fireEvent.click(screen.getByRole('radio', { name: 'Globe' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Galactic' }));
    expect(container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    // Turning the map asks the server for nothing.
    expect(asked.calls).toHaveLength(3);
  });

  describe('the minimum field', () => {
    it('asks once the person stops typing, and offers the way back', async () => {
      show();
      await waitFor(() => expect(legend()).toContain('30 min or more'));
      vi.useFakeTimers();
      const field = screen.getByLabelText('Minimum minutes') as HTMLInputElement;
      for (const text of ['1', '12', '120']) {
        fireEvent.change(field, { target: { value: text } });
        act(() => { vi.advanceTimersByTime(MINIMUM_SETTLE_MS - 100); });
      }
      expect(asked.calls).toHaveLength(1);
      act(() => { vi.advanceTimersByTime(100); });
      vi.useRealTimers();
      await waitFor(() => expect(legend()).toContain('2 h or more'));
      expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }, { rig: 'rig-1', span: 'now', minimum_minutes: 120 }]);
      // Enter sends at once; text that is not a time sends nothing.
      fireEvent.change(field, { target: { value: '' } });
      fireEvent.blur(field);
      fireEvent.change(field, { target: { value: '45' } });
      fireEvent.keyDown(field, { key: 'Enter' });
      await waitFor(() => expect(legend()).toContain('45 min or more'));
      expect(asked.calls).toHaveLength(3);
      fireEvent.click(screen.getByRole('button', { name: "Use rig's 30 min" }));
      await waitFor(() => expect(legend()).toContain('30 min or more'));
      expect(screen.queryByRole('button', { name: /Use rig's/ })).toBeNull();
    });
  });

  it('shows a clicked place with its nights and sends it to Framing only on a click of Frame this', async () => {
    const { container, onFrame } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    expect(screen.queryByRole('group', { name: 'Picked place' })).toBeNull();
    const svg = container.querySelector('.sky-map') as SVGSVGElement;
    vi.spyOn(svg, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, width: 1000, height: 540, right: 1000, bottom: 540, x: 0, y: 0, toJSON: () => ({}) });
    // The middle of the map: 12h on the equator, in view on 12 nights.
    fireEvent.click(svg, { clientX: 500, clientY: 270 });
    const card = screen.getByRole('group', { name: 'Picked place' });
    expect(card.textContent).toContain('12h 00m, +0° 00′');
    // Until the server has counted the place, the card claims nothing.
    expect(card.textContent).toContain('Counting its nights in view…');
    await waitFor(() => expect(card.textContent).toContain('In view on 12 of the 31 nights counted. Between the lines: in view on some nights.'));
    expect(asked.places.at(-1)?.places).toHaveLength(2);
    expect(container.querySelector('.sky-mark-pick')).not.toBeNull();
    // Picking changes nothing in the plan.
    expect(onFrame).not.toHaveBeenCalled();
    fireEvent.click(within(card).getByRole('button', { name: 'Frame this' }));
    expect(onFrame).toHaveBeenCalledTimes(1);
    expect(onFrame.mock.calls[0][0].name).toBe('12h 00m, +0° 00′');
    expect(onFrame.mock.calls[0][0].center.ra_degrees).toBeCloseTo(180, 4);
    expect(onFrame.mock.calls[0][0].center.dec_degrees).toBeCloseTo(0, 4);
    fireEvent.click(within(card).getByRole('button', { name: 'Clear' }));
    expect(screen.queryByRole('group', { name: 'Picked place' })).toBeNull();
    expect(container.querySelector('.sky-mark-pick')).toBeNull();
  });

  it('shows an object found by name, and says why it cannot be framed when Framing is held', async () => {
    const { onFrame } = show({ frameBlocked: 'Locked panels prevent changes to the center and camera angle. Unlock them to move the whole mosaic.' });
    await waitFor(() => expect(legend()).toContain('31 nights'));
    fireEvent.change(screen.getByLabelText('Find an object'), { target: { value: 'M 31' } });
    fireEvent.click(await screen.findByText('Andromeda Galaxy'));
    const card = await screen.findByRole('group', { name: 'Picked place' });
    expect(card.textContent).toContain('M 31');
    await waitFor(() => expect(card.textContent).toContain('In view on 31 of the 31 nights counted. Inside the every-night line.'));
    expect(within(card).getByRole('button', { name: 'Frame this' })).toBeDisabled();
    expect(card.textContent).toContain('Locked panels prevent changes');
    expect(onFrame).not.toHaveBeenCalled();
  });

  it('says a place far south is never in view', async () => {
    const { container } = show({ target: { name: 'Carina', center: { ra_degrees: 160, dec_degrees: -60 } } });
    await waitFor(() => expect(legend()).toContain('31 nights'));
    await waitFor(() => expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 0 of the 31 nights counted. Outside both lines: never in view for long enough.'));
    expect(container.querySelector('.sky-mark-target')?.getAttribute('data-mark')).toBe('Carina');
  });

  it('counts a place on the edge for itself, not from the grid node beside it', async () => {
    // A target at −29.2°: inside the dashed line at −30°, though the node
    // nearest it on the 2° grid, at −30°, could read either way and the one
    // at −28° is another place. The server counts the place itself.
    show({ target: { name: 'Edge', center: { ra_degrees: 100, dec_degrees: -29.2 } } });
    await waitFor(() => expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 12 of the 31 nights counted. Between the lines: in view on some nights.'));
    expect(asked.places.at(-1)?.places).toEqual([{ ra_degrees: 100, dec_degrees: -29.2 }]);
  });

  it('says so when the nights of a place cannot be counted, and claims no number', async () => {
    asked.placesFail = true;
    show();
    await waitFor(() => expect(screen.getByTestId('plan-sky-target').textContent).toContain('Its nights in view could not be counted.'));
    expect(screen.getByTestId('plan-sky-target').textContent).not.toMatch(/In view on/);
  });

  it('keeps the picture and the minimum on screen, dimmed, while another time frame loads', async () => {
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    let arrive: (value: DirectorVisibleSky) => void = () => {};
    asked.answer = (request) => new Promise((resolve) => { arrive = () => resolve(view(request)); });
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    // The month's picture stays, marked busy, and the legend says which is which.
    await waitFor(() => expect(legend()).toMatch(/^Now is shown while Year loads\. C925: in view 30 min or more · 31 nights/));
    expect(screen.getByRole('region', { name: 'Sky in view' }).getAttribute('aria-busy')).toBe('true');
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('31');
    expect(container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    expect((screen.getByLabelText('Minimum minutes') as HTMLInputElement).value).toBe('30');
    expect(screen.getByRole('radio', { name: 'Year' }).getAttribute('aria-checked')).toBe('true');
    await act(async () => { arrive(view({ rig: 'rig-1', span: 'year' })); });
    await waitFor(() => expect(legend()).toMatch(/^C925: in view 30 min or more · 365 nights from tonight/));
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('365');
    expect(screen.getByRole('region', { name: 'Sky in view' }).getAttribute('aria-busy')).toBe('false');
    // Another rig's picture is never shown for this one: the map clears.
    asked.answer = () => new Promise(() => {});
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-2' } });
    await waitFor(() => expect(legend()).toBe('Working out the sky in view…'));
    expect(container.querySelector('.sky-visible-shade')).toBeNull();
    // Back on the first rig, its minimum is known before the answer is.
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-1' } });
    expect((screen.getByLabelText('Minimum minutes') as HTMLInputElement).value).toBe('30');
  });

  it('reads the picture again after a save that can change it, and after no other', async () => {
    const { client, again } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    await waitFor(() => expect(asked.places).toHaveLength(1));
    expect(asked.calls).toHaveLength(1);
    // A save of something else, such as a name lookup or a plan's exposures.
    await save(client, ['director', 'something-else']);
    await save(client, []);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(asked.calls).toHaveLength(1);
    expect(asked.places).toHaveLength(1);
    // A save of the rig's site, horizon, limits or minimum time.
    await save(client, [...SKY_INPUTS_MUTATION]);
    await waitFor(() => expect(asked.calls).toHaveLength(2));
    await waitFor(() => expect(asked.places).toHaveLength(2));
    // Behind another tab it asks for nothing, and catches up when shown.
    again({ shown: false });
    await save(client, [...SKY_INPUTS_MUTATION]);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(asked.calls).toHaveLength(2);
    again({ shown: true });
    await waitFor(() => expect(asked.calls).toHaveLength(3));
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'now' });
  });

  it('says a plan has no rig yet and leads to the Rigs tab', () => {
    const { onRigs, container } = show({ rigs: [] });
    expect(screen.getByRole('status').textContent).toBe('This plan has no rig yet. Add a rig on the Rigs tab');
    expect(asked.calls).toEqual([]);
    expect(container.querySelector('.sky-map')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Add a rig on the Rigs tab' }));
    expect(onRigs).toHaveBeenCalled();
  });

  it("explains a rig with no site and opens its Rig setup, and says when a request fails", async () => {
    asked.answer = async () => { throw new Error('C925 has no location. Choose a Planning site in Rig setup.', { cause: { isAxiosError: true, response: { status: 422, data: { error: 'C925 has no location. Choose a Planning site in Rig setup.' } } } }); };
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('C925 has no location. Choose a Planning site in Rig setup.'));
    fireEvent.click(screen.getByRole('button', { name: 'Open Rig setup' }));
    expect(openSettings).toHaveBeenCalledWith('rigs', { databaseId: 'c925' });
    // The map stays, with no shading or lines to mislead.
    expect(container.querySelector('.sky-map')).not.toBeNull();
    expect(container.querySelector('.sky-visible')).toBeNull();
    expect(container.querySelector('.sky-visible-shade')).toBeNull();
    // Another rig's request simply failing.
    asked.answer = async () => { throw new Error('boom'); };
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-2' } });
    await waitFor(() => expect(legend()).toBe('The sky in view could not be worked out. '));
    expect(screen.queryByRole('button', { name: 'Open Rig setup' })).toBeNull();
  });

  it('shows a month of the year at a time under a slider, from the year already fetched', async () => {
    // A target on the equator: in view while the southern band is.
    const { container } = show({ target: { name: 'Equator', center: { ra_degrees: 38.2, dec_degrees: 0 } } });
    await waitFor(() => expect(legend()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    const slider = (await screen.findByRole('slider', { name: 'Time of year' })) as HTMLInputElement;
    // Season asks for the year, once.
    await waitFor(() => expect(slider.disabled).toBe(false));
    expect(asked.calls).toEqual([{ rig: 'rig-1', span: 'now' }, { rig: 'rig-1', span: 'year' }]);
    // 49 places a week apart: arrow keys step one, Home and End reach the ends.
    expect([slider.min, slider.max, slider.step, slider.value]).toEqual(['0', '48', '1', '0']);
    expect(slider.getAttribute('aria-valuetext')).toBe(`31 nights from ${formatNight('2026-09-24')}`);
    expect(container.querySelector('.plan-sky-season-nights')?.textContent).toBe(`31 nights from ${formatNight('2026-09-24')}`);
    // The first place is the month Now shows, from the weekly nights.
    expect(legend()).toContain(`C925: in view 30 min or more · 31 nights from tonight, ${formatNight('2026-09-24')}, to ${formatNight('2026-10-24')}, one a week measured ·`);
    expect(legend()).toContain('In view every night: 76% of the sky, declination −31° to +90°. On at least one night: 76% of the sky, declination −31° to +90°.');
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('31');
    expect(container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).toMatch(/^M/);
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 31 of the 31 nights counted. Inside the every-night line.');
    expect(screen.getByLabelText('How to read the map').textContent).toContain('Season shows 31 nights of the year at a time.');
    // The months run along the slider.
    expect(container.querySelectorAll('.plan-sky-season-months span').length).toBeGreaterThanOrEqual(10);

    // Seven weeks on, the southern band is in view on the first 15 nights only.
    const map = screen.getByRole('img', { name: /^Map of the sky/ });
    const before = container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d');
    fireEvent.change(slider, { target: { value: '7' } });
    expect(slider.getAttribute('aria-valuetext')).toBe(`31 nights from ${formatNight('2026-11-12')}`);
    expect(legend()).toContain(`31 nights from ${formatNight('2026-11-12')} to ${formatNight('2026-12-12')}, one a week measured`);
    expect(legend()).not.toContain('from tonight');
    expect(legend()).toContain('In view every night: 19% of the sky, declination +39° to +90°. On at least one night: 76% of the sky, declination −31° to +90°.');
    expect(map.getAttribute('aria-label')).toContain(`31 nights from ${formatNight('2026-11-12')}`);
    expect(container.querySelector('.sky-visible-line.sky-visible-every')?.getAttribute('d')).not.toBe(before);
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 15 of the 31 nights counted. Between the lines: in view on some nights.');
    // Page Up and Page Down move about a month.
    fireEvent.keyDown(slider, { key: 'PageUp' });
    expect(slider.value).toBe('11');
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 0 of the 31 nights counted. Outside both lines: never in view for long enough.');
    fireEvent.keyDown(slider, { key: 'PageDown' });
    fireEvent.keyDown(slider, { key: 'PageDown' });
    expect(slider.value).toBe('3');
    fireEvent.keyDown(slider, { key: 'PageDown' });
    expect(slider.value).toBe('0');
    // The last place ends with the year.
    fireEvent.change(slider, { target: { value: '48' } });
    expect(legend()).toContain(`31 nights from ${formatNight('2027-08-24')} to ${formatNight('2027-09-23')}, one a week measured`);

    // None of it asked the server for anything, about the picture or a place.
    expect(asked.calls).toHaveLength(2);
    expect(asked.places.every((request) => request.span === 'now')).toBe(true);
    // Year and Season are one answer: going between them asks for nothing.
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(legend()).toContain('365 nights'));
    expect(screen.queryByRole('slider')).toBeNull();
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    expect((screen.getByRole('slider', { name: 'Time of year' }) as HTMLInputElement).value).toBe('48');
    expect(asked.calls).toHaveLength(2);
    // Another rig is another year: the slider starts again.
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'rig-2' } });
    await waitFor(() => expect(legend()).toContain('RC51: in view'));
    expect((screen.getByRole('slider', { name: 'Time of year' }) as HTMLInputElement).value).toBe('0');
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-2', span: 'year' });
  });

  it('draws the cover small while the slider moves and whole once it rests', async () => {
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Season' }));
    const slider = (await screen.findByRole('slider', { name: 'Time of year' })) as HTMLInputElement;
    await waitFor(() => expect(slider.disabled).toBe(false));
    // A canvas that records the size of each picture painted.
    const sizes: number[] = [];
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(function (this: HTMLCanvasElement) {
      sizes.push(this.width);
      return null;
    });
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'requestAnimationFrame', 'cancelAnimationFrame'] });
    try {
      fireEvent.change(slider, { target: { value: '5' } });
      act(() => { vi.advanceTimersByTime(20); });
      expect(sizes.at(-1)).toBe(160);
      act(() => { vi.advanceTimersByTime(SEASON_SETTLE_MS + 5); });
      act(() => { vi.advanceTimersByTime(40); });
      expect(sizes.at(-1)).toBe(400);
    } finally {
      vi.useRealTimers();
    }
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('31');
  });
});
