import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import {
  penaltyParamsOf,
  scoringDefaults,
  SCORING_DEFAULTS as DEFAULTS,
  setScoringDefaults,
  useScoringPreferences,
  useViewScoring,
} from '../useScoringPreferences';
import { getDisplayPreferences } from '../useDisplayPreferences';
import { keepScoringInScope, useDbProjectTarget, useUrlParams } from '../useUrlState';
import { imageDetailPath } from '../../utils/imageDetailRoutes';

function inRouter(entry: string) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <MemoryRouter initialEntries={[entry]}>{children}</MemoryRouter>;
  };
}

/** The Scoring control's hook, with the link it writes to. */
function renderViewScoring(entry = '/sequence?db=main&project=1&target=1') {
  return renderHook(() => ({ view: useViewScoring(), search: useLocation().search }), {
    wrapper: inRouter(entry),
  });
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

describe('a view\'s scoring', () => {
  beforeEach(() => setScoringDefaults({ ...DEFAULTS }));
  afterEach(() => setScoringDefaults({ ...DEFAULTS }));

  it('starts from the saved defaults', () => {
    setScoringDefaults({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 2.55 });
    const { result } = renderViewScoring();
    expect(result.current.view.scoring).toEqual({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 2.55 });
    expect(result.current.view.changed).toBe(false);
  });

  it('lets the link win over the defaults', () => {
    setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.55 });
    const { result } = renderHook(() => useScoringPreferences(), {
      wrapper: inRouter('/grid?db=main&maxHfr=4&penaltyPointing=0'),
    });
    expect(result.current).toEqual({ ...DEFAULTS, pointing: 0, hfrRejectAbove: 4 });
  });

  it('applies a change to the view alone, as the link', () => {
    setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.55 });
    const { result } = renderViewScoring();
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
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, hfrRejectAbove: 2.55 });
  });

  it('saves the view as the default and leaves a clean link', () => {
    const { result } = renderViewScoring('/sequence?db=main&penaltySatellite=0&maxHfr=3');
    expect(result.current.view.changed).toBe(true);
    act(() => result.current.view.saveAsDefault(result.current.view.scoring));
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 3 });
    expect(getDisplayPreferences()).toMatchObject({
      scoringPenaltySatellite: 0,
      scoringHfrRejectAbove: 3,
    });
    expect(result.current.search).toBe('?db=main');
    expect(result.current.view.scoring).toEqual({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 3 });
    expect(result.current.view.changed).toBe(false);
  });

  it('resets the view to the defaults', () => {
    setScoringDefaults({ ...DEFAULTS, temporal: 0.5 });
    const { result } = renderViewScoring('/sequence?db=main&penaltyTemporal=2&minStars=40');
    expect(result.current.view.scoring).toMatchObject({ temporal: 2, starCountRejectBelow: 40 });
    act(() => result.current.view.resetToDefault());
    expect(result.current.search).toBe('?db=main');
    expect(result.current.view.scoring).toEqual({ ...DEFAULTS, temporal: 0.5 });
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, temporal: 0.5 });
  });

  it('keeps one snapshot while the values stay the same', () => {
    const { result, rerender } = renderViewScoring();
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
