import { describe, expect, it } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import {
  penaltyParamsOf,
  SCORING_DEFAULTS as DEFAULTS,
  useScoringDefaults,
  useScoringPreferences,
  useViewScoring,
} from '../useScoringPreferences';
import { getDisplayPreferences } from '../useDisplayPreferences';
import { AccessContext } from '../../auth/access';
import {
  deliverScoringDefaults,
  scoringServer,
  testQueryClient,
  VIEWER_ACCESS,
  waitForScoringDefaults,
} from '../../test/scoringDefaults';
import { keepScoringInScope, useDbProjectTarget, useUrlParams } from '../useUrlState';
import { imageDetailPath } from '../../utils/imageDetailRoutes';

function inRouter(entry: string, client: QueryClient = testQueryClient()) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[entry]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

/** The Scoring control's hook, with the link it writes to, once the
 *  server's defaults have arrived. */
async function renderViewScoring(entry = '/sequence?db=main&project=1&target=1') {
  const client = testQueryClient();
  const view = renderHook(() => ({ view: useViewScoring(), search: useLocation().search }), {
    wrapper: inRouter(entry, client),
  });
  await waitForScoringDefaults(client);
  return { client, ...view };
}

describe('scoring request params', () => {
  it('omits the calibrated values entirely', () => {
    expect(penaltyParamsOf(DEFAULTS)).toEqual({});
  });

  it('sends only what differs from the calibrated values', () => {
    expect(penaltyParamsOf({ ...DEFAULTS, satellite: 0, temporal: 1.5 })).toEqual({
      penalty_satellite: 0,
      penalty_temporal: 1.5,
    });
    expect(penaltyParamsOf({ ...DEFAULTS, hfrRise: 2, starLoss: 0 })).toEqual({
      penalty_hfr_rise: 2,
      penalty_star_loss: 0,
    });
    expect(penaltyParamsOf({ ...DEFAULTS, hfrRejectAbove: 3.5, starCountRejectBelow: 50 })).toEqual({
      hfr_reject_above: 3.5,
      star_count_reject_below: 50,
    });
  });
});

describe('the server\'s scoring defaults', () => {
  it('are the built-in values until the server\'s arrive', async () => {
    scoringServer.set({ ...DEFAULTS, satellite: 0 });
    const { result } = renderHook(() => useScoringDefaults(), { wrapper: inRouter('/') });
    expect(result.current).toEqual(DEFAULTS);
    await waitFor(() => expect(result.current).toEqual({ ...DEFAULTS, satellite: 0 }));
  });

  it('follow defaults saved elsewhere', async () => {
    const client = testQueryClient();
    const { result } = renderHook(() => useScoringDefaults(), { wrapper: inRouter('/', client) });
    await waitForScoringDefaults(client);
    act(() => deliverScoringDefaults(client, { ...DEFAULTS, hfrRejectAbove: 2.2 }));
    await waitFor(() => expect(result.current.hfrRejectAbove).toBe(2.2));
  });
});

describe('a view\'s scoring', () => {
  it('starts from the server\'s defaults', async () => {
    scoringServer.set({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 2.55 });
    const { result } = await renderViewScoring();
    await waitFor(() =>
      expect(result.current.view.scoring).toEqual({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 2.55 }));
    expect(result.current.view.changed).toBe(false);
    expect(result.current.view.canSaveDefault).toBe(true);
  });

  it('lets the link win over the defaults', async () => {
    scoringServer.set({ ...DEFAULTS, hfrRejectAbove: 2.55 });
    const client = testQueryClient();
    const { result } = renderHook(() => useScoringPreferences(), {
      wrapper: inRouter('/grid?db=main&maxHfr=4&penaltyPointing=0', client),
    });
    await waitForScoringDefaults(client);
    expect(result.current).toEqual({ ...DEFAULTS, pointing: 0, hfrRejectAbove: 4 });
  });

  it('applies a change to the view alone, as the link', async () => {
    scoringServer.set({ ...DEFAULTS, hfrRejectAbove: 2.55 });
    const { result } = await renderViewScoring();
    act(() => result.current.view.setScoring({
      ...result.current.view.scoring,
      starCountRejectBelow: 505,
    }));
    expect(new URLSearchParams(result.current.search).get('minStars')).toBe('505');
    // The link names every value, so it scores the same for anyone.
    expect(new URLSearchParams(result.current.search).get('maxHfr')).toBe('2.55');
    expect(new URLSearchParams(result.current.search).get('penaltySatellite')).toBe('1');
    expect(result.current.search).toContain('db=main');
    expect(result.current.view.scoring.starCountRejectBelow).toBe(505);
    expect(result.current.view.changed).toBe(true);
    // The defaults have not moved.
    expect(scoringServer.saves).toEqual([]);
  });

  it('saves the view as the server\'s default and leaves a clean link', async () => {
    const before = getDisplayPreferences();
    const { result } = await renderViewScoring('/sequence?db=main&penaltySatellite=0&maxHfr=3');
    expect(result.current.view.changed).toBe(true);
    act(() => result.current.view.saveAsDefault(result.current.view.scoring));
    await waitFor(() => expect(result.current.search).toBe('?db=main'));
    expect(scoringServer.saves).toEqual([{ ...DEFAULTS, satellite: 0, hfrRejectAbove: 3 }]);
    // The server keeps them, not this person's display preferences.
    expect(getDisplayPreferences()).toEqual(before);
    expect(result.current.view.scoring).toEqual({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 3 });
    expect(result.current.view.changed).toBe(false);
  });

  it('keeps the view\'s link when the save fails', async () => {
    scoringServer.failure = { error: 'The settings file is read-only' };
    const { result } = await renderViewScoring('/sequence?db=main&penaltySatellite=0&maxHfr=3');
    act(() => result.current.view.saveAsDefault(result.current.view.scoring));
    await waitFor(() =>
      expect(result.current.view.saveDefaultError?.message).toBe('The settings file is read-only'));
    expect(result.current.view.savingDefault).toBe(false);
    expect(result.current.search).toBe('?db=main&penaltySatellite=0&maxHfr=3');
    expect(result.current.view.changed).toBe(true);
    expect(scoringServer.get()).toEqual(DEFAULTS);
  });

  it('tells a viewer they cannot save the defaults', async () => {
    const client = testQueryClient();
    const Router = inRouter('/sequence?db=main', client);
    const { result } = renderHook(() => useViewScoring(), {
      wrapper: ({ children }: { children: ReactNode }) => (
        <AccessContext.Provider value={VIEWER_ACCESS}><Router>{children}</Router></AccessContext.Provider>
      ),
    });
    await waitForScoringDefaults(client);
    expect(result.current.canSaveDefault).toBe(false);
  });

  it('resets the view to the defaults', async () => {
    scoringServer.set({ ...DEFAULTS, temporal: 0.5 });
    const { result } = await renderViewScoring('/sequence?db=main&penaltyTemporal=2&minStars=40');
    expect(result.current.view.scoring).toMatchObject({ temporal: 2, starCountRejectBelow: 40 });
    act(() => result.current.view.resetToDefault());
    expect(result.current.search).toBe('?db=main');
    expect(result.current.view.scoring).toEqual({ ...DEFAULTS, temporal: 0.5 });
    expect(scoringServer.saves).toEqual([]);
  });

  it('keeps one snapshot while the values stay the same', async () => {
    const { result, rerender } = await renderViewScoring();
    const first = result.current.view.scoring;
    rerender();
    expect(result.current.view.scoring).toBe(first);
  });
});

describe('a view\'s scoring and its scope', () => {
  const SCORED = '/sequence?db=main&project=1&target=4&penaltySatellite=0&penaltyPointing=1'
    + '&penaltyTemporal=1&maxHfr=off&minStars=40';

  function renderScope(entry = SCORED) {
    return renderHook(() => ({
      scope: useDbProjectTarget(),
      params: useUrlParams(),
      search: useLocation().search,
    }), { wrapper: inRouter(entry) });
  }
  const has = (search: string) => new URLSearchParams(search).has('minStars');

  it('stays with another target of the same project', () => {
    const { result } = renderScope();
    act(() => result.current.scope.setTargetId(7));
    expect(result.current.search).toContain('target=7');
    expect(has(result.current.search)).toBe(true);
    act(() => result.current.scope.setDbProjectTarget('main', 1, 9));
    expect(has(result.current.search)).toBe(true);
    // And opens with a frame's detail.
    const detail = imageDetailPath(5, new URLSearchParams(result.current.search), 'sequence');
    expect(new URLSearchParams(detail.split('?')[1]).get('minStars')).toBe('40');
  });

  it('goes when the project or the database changes', () => {
    const { result } = renderScope();
    act(() => result.current.scope.setProjectId(2));
    expect(result.current.search).toContain('project=2');
    expect(has(result.current.search)).toBe(false);

    const other = renderScope();
    act(() => other.result.current.scope.setDbProjectTarget('second', 1, null));
    expect(has(other.result.current.search)).toBe(false);

    const params = renderScope();
    act(() => params.result.current.params.updateParams({ project: null }));
    expect(has(params.result.current.search)).toBe(false);
  });

  it('goes with a link built for another scope', () => {
    const from = SCORED.split('?')[1];
    const same = new URLSearchParams(from);
    same.set('target', '9');
    expect(keepScoringInScope(from, same).get('minStars')).toBe('40');
    const elsewhere = new URLSearchParams(from);
    elsewhere.set('project', '2');
    expect(keepScoringInScope(from, elsewhere).has('minStars')).toBe(false);
    expect(keepScoringInScope(from, elsewhere).get('db')).toBe('main');
    const otherDb = new URLSearchParams(from);
    otherDb.set('db', 'second');
    for (const name of [
      'penaltySatellite', 'penaltyPointing', 'penaltyTemporal', 'penaltyHfrRise', 'penaltyStarLoss',
      'maxHfr', 'minStars',
    ]) {
      expect(keepScoringInScope(from, otherDb).has(name)).toBe(false);
    }
  });
});
