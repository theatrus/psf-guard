import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { formatNightDate } from '../nights/nightsModel';
import PlanSky, { MINIMUM_SETTLE_MS } from '../director/PlanSky';

// The map is a heavy thing to draw; a busy machine needs more than a second.
configure({ asyncUtilTimeout: 5000 });

const asked = vi.hoisted(() => ({ calls: [] as DirectorVisibleSkyRequest[], answer: (request: DirectorVisibleSkyRequest): Promise<unknown> => Promise.reject(new Error(`unset ${request.rig}`)) }));
const openSettings = vi.hoisted(() => vi.fn());
vi.mock('../../utils/settingsIntent', () => ({ openSettings }));
vi.mock('../../api/client', () => ({
  apiClient: {
    getDirectorVisibleSky: (request: DirectorVisibleSkyRequest) => { asked.calls.push(request); return asked.answer(request); },
    searchDirectorNames: async (query: string) => ({ query, local: { available: true, items: query.toLowerCase().startsWith('m 3') ? [{ name: 'M 31', common_name: 'Andromeda Galaxy', kind: 'galaxy', ra_degrees: 10.68, dec_degrees: 41.27, matched: 'M 31', source: 'Seiza object catalog' }] : [] }, online: null, online_state: 'skipped', online_cached: false }),
    resolveDirectorName: async (name: string) => { throw new Error(`No catalog knows "${name}"`); },
  },
}));

/** Counts as the server packs them: in view every night north of +40°,
 *  on 12 nights down to −30°, never south of that. */
const counts = (nights: number) => {
  const bytes = new Uint8Array(180 * 91);
  for (let row = 0; row < 91; row += 1) bytes.fill(-90 + row * 2 >= 40 ? nights : -90 + row * 2 >= -30 ? 12 : 0, row * 180, (row + 1) * 180);
  let text = '';
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
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
    grid: { degrees: 2, columns: 180, rows: 91 }, counts: counts(year ? 53 : 31),
    every_night: { lines: [ring(40)], sky_fraction: 0.18, declinations: [39.6, 90] },
    any_night: { lines: [ring(-30)], sky_fraction: 0.75, declinations: [-30.4, 90] },
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
  const rendered = render(<QueryClientProvider client={client}><PlanSky rigs={rigs} target={target} frameBlocked={null} shown {...handlers} {...props} /></QueryClientProvider>);
  return { ...handlers, ...rendered };
}
const legend = () => screen.getByRole('status').textContent ?? '';

describe("the plan's Sky tab", () => {
  beforeEach(() => { asked.calls.length = 0; asked.answer = async (request) => view(request); openSettings.mockClear(); });
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
    expect(legend()).toBe(`C925: in view 30 min or more · 31 nights, ${formatNightDate('2026-09-24')} to ${formatNightDate('2026-10-24')} · the site's custom horizon, at least 25°. In view every night: 18% of the sky, declination +40° to +90°. On at least one night: 75% of the sky, declination −30° to +90°.`);
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
    expect(key.textContent).toBe('No nightEvery nightEdge of the sky in view every nightEdge of the sky in view on any nightMarks point into the sky in view.');
    // The plan's target is marked, with the nights it is in view.
    expect(container.querySelector('.sky-mark-target')?.getAttribute('data-mark')).toBe('IC 1805');
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('Target: IC 1805');
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 31 of the next 31 nights. Inside the every-night line.');
    // A fetch marks the tab busy; it adds no words.
    expect(screen.getByRole('region', { name: 'Sky in view' }).getAttribute('aria-busy')).toBe('false');
  });

  it('counts a night a week for a year, and takes another rig, map shape and frame', async () => {
    const { container } = show();
    await waitFor(() => expect(legend()).toContain('31 nights'));
    fireEvent.click(screen.getByRole('radio', { name: 'Year' }));
    await waitFor(() => expect(legend()).toContain('one night a week for a year'));
    expect(asked.calls.at(-1)).toEqual({ rig: 'rig-1', span: 'year' });
    expect(container.querySelector('.sky-visible-shade')?.getAttribute('data-nights')).toBe('53');
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 53 of the 53 nights sampled over the year.');
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
    expect(card.textContent).toContain('In view on 12 of the next 31 nights. Between the lines: in view on some nights.');
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
    expect(card.textContent).toContain('In view on 31 of the next 31 nights. Inside the every-night line.');
    expect(within(card).getByRole('button', { name: 'Frame this' })).toBeDisabled();
    expect(card.textContent).toContain('Locked panels prevent changes');
    expect(onFrame).not.toHaveBeenCalled();
  });

  it('says a place far south is never in view', async () => {
    const { container } = show({ target: { name: 'Carina', center: { ra_degrees: 160, dec_degrees: -60 } } });
    await waitFor(() => expect(legend()).toContain('31 nights'));
    expect(screen.getByTestId('plan-sky-target').textContent).toContain('In view on 0 of the next 31 nights. Outside both lines: never in view for long enough.');
    expect(container.querySelector('.sky-mark-target')?.getAttribute('data-mark')).toBe('Carina');
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
});
