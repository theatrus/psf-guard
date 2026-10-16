import { beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import {
  getDisplayPreferences,
  setDetailSection,
  setDisplayPreferences,
  setPreferencesAccount,
  useDisplayPreferences,
} from '../useDisplayPreferences';

/** The Scoring control's defaults as built in. */
const BUILT_IN_SCORING = {
  scoringPenaltySatellite: 1,
  scoringPenaltyPointing: 1,
  scoringPenaltyTemporal: 1,
  scoringPenaltyHfrRise: 1,
  scoringPenaltyStarLoss: 1,
  scoringHfrRejectAbove: null,
  scoringStarCountRejectBelow: null,
};

describe('display preferences', () => {
  beforeEach(() => {
    window.localStorage.removeItem('psf-guard.display-preferences');
    setDisplayPreferences({
      showNightChip: true,
      showAllChip: true, showCalibrationChip: true,
      advanceOnGrade: true,
      projectPickerGrouping: 'activity',
      libraryDensity: 'compact',
      detailSections: {},
    });
  });

  it('defaults to showing both chip types and advancing after a grade', () => {
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current).toEqual({
      showNightChip: true,
      showAllChip: true, showCalibrationChip: true,
      advanceOnGrade: true,
      projectPickerGrouping: 'activity',
      libraryDensity: 'compact',
      detailSections: {},
      imageOrder: 'oldest',
      timeMarks: 'nights-hours',
      imageGrouping: 'filter',
      imageGroupingAllProjects: 'project',
      gridThumbnailSize: 300,
      sequenceThumbnailSize: 150,
      ...BUILT_IN_SCORING,
    });
  });

  it('remembers each detail section the viewer opened or folded', () => {
    const { result } = renderHook(() => useDisplayPreferences());
    act(() => setDetailSection('calibration', 'open'));
    act(() => setDetailSection('sky', 'closed'));
    expect(result.current.detailSections).toEqual({ calibration: 'open', sky: 'closed' });
    expect(
      JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!).detailSections,
    ).toEqual({ calibration: 'open', sky: 'closed' });

    // A caller that changes a chip, and does not name the sections, keeps them.
    act(() => setDisplayPreferences({ ...result.current, showNightChip: false, detailSections: undefined }));
    expect(result.current.detailSections).toEqual({ calibration: 'open', sky: 'closed' });
  });

  it('drops stored section choices it does not know', () => {
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ detailSections: { calibration: 'closed', sky: 'sideways', stacks: 'open' } }),
    );
    window.dispatchEvent(
      new StorageEvent('storage', { key: 'psf-guard.display-preferences' }),
    );
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.detailSections).toEqual({ calibration: 'closed' });
  });

  it('toggles each chip type independently', () => {
    act(() => {
      setDisplayPreferences({
        showNightChip: false,
        showAllChip: true, showCalibrationChip: true,
        advanceOnGrade: true,
        projectPickerGrouping: 'activity',
        libraryDensity: 'compact',
      });
    });
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showNightChip).toBe(false);
    expect(result.current.showAllChip).toBe(true);
  });

  it('updates every subscriber when one toggle flips', () => {
    // Two hook instances stand in for the grid toolbar and the Sequence
    // toolbar: one shared preference, so flipping either flips both.
    const grid = renderHook(() => useDisplayPreferences());
    const sequence = renderHook(() => useDisplayPreferences());
    act(() => {
      setDisplayPreferences({
        showNightChip: true,
        showAllChip: false, showCalibrationChip: true,
        advanceOnGrade: true,
        projectPickerGrouping: 'activity',
        libraryDensity: 'compact',
      });
    });
    expect(grid.result.current.showAllChip).toBe(false);
    expect(sequence.result.current.showAllChip).toBe(false);
  });

  it('persists the choice to localStorage', () => {
    act(() => {
      setDisplayPreferences({
        showNightChip: false,
        showAllChip: false, showCalibrationChip: true,
        advanceOnGrade: false,
        projectPickerGrouping: 'database',
        libraryDensity: 'compact',
      });
    });
    expect(
      JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!),
    ).toEqual({
      showNightChip: false,
      showAllChip: false, showCalibrationChip: true,
      advanceOnGrade: false,
      projectPickerGrouping: 'database',
      libraryDensity: 'compact',
      detailSections: {},
      imageOrder: 'oldest',
      timeMarks: 'nights-hours',
      imageGrouping: 'filter',
      imageGroupingAllProjects: 'project',
      gridThumbnailSize: 300,
      sequenceThumbnailSize: 150,
      ...BUILT_IN_SCORING,
    });
  });

  it('remembers the image order, and reads oldest first when none or a bad one is stored', () => {
    const base = {
      showNightChip: true,
      showAllChip: true, showCalibrationChip: true,
      advanceOnGrade: true,
      projectPickerGrouping: 'activity',
      libraryDensity: 'compact',
    } as const;
    act(() => setDisplayPreferences({ ...base, imageOrder: 'newest' }));
    expect(renderHook(() => useDisplayPreferences()).result.current.imageOrder).toBe('newest');
    expect(JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!).imageOrder)
      .toBe('newest');

    act(() => setDisplayPreferences({ ...base, imageOrder: 'sideways' as unknown as 'newest' }));
    expect(renderHook(() => useDisplayPreferences()).result.current.imageOrder).toBe('oldest');

    // A preference saved before the order existed.
    act(() => setDisplayPreferences(base));
    expect(renderHook(() => useDisplayPreferences()).result.current.imageOrder).toBe('oldest');
  });

  it('falls back to the default on malformed stored values', () => {
    setDisplayPreferences({
      showNightChip: 'yes' as unknown as boolean,
      showAllChip: true, showCalibrationChip: true,
      advanceOnGrade: true,
      projectPickerGrouping: 'sideways' as unknown as 'activity',
      libraryDensity: 'huge' as unknown as 'compact',
    });
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showNightChip).toBe(true);
    expect(result.current.projectPickerGrouping).toBe('activity');
  });

  it('honors the earlier single-switch stored form', () => {
    // A browser that stored the one-switch build's "off" keeps both chips
    // off after upgrading rather than silently turning them back on.
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ showSecondaryScore: false }),
    );
    window.dispatchEvent(
      new StorageEvent('storage', { key: 'psf-guard.display-preferences' }),
    );
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showNightChip).toBe(false);
    expect(result.current.showAllChip).toBe(false);
  });
});

describe('display preferences per account', () => {
  const base = {
    showNightChip: true,
    showAllChip: true, showCalibrationChip: true,
    advanceOnGrade: true,
    projectPickerGrouping: 'activity',
    libraryDensity: 'compact',
  } as const;

  it('keeps a signed-in user\'s copy apart from the browser\'s own', () => {
    window.localStorage.clear();
    setPreferencesAccount(null);
    act(() => setDisplayPreferences({ ...base, imageOrder: 'newest' }));

    // A first sign-in starts from what the browser chose...
    act(() => setPreferencesAccount('alice'));
    expect(getDisplayPreferences().imageOrder).toBe('newest');
    // ...and the user's changes stay theirs.
    act(() => setDisplayPreferences({ ...base, imageOrder: 'oldest', imageGrouping: 'date' }));
    expect(window.localStorage.getItem('psf-guard.display-preferences@alice')).toContain('"date"');

    act(() => setPreferencesAccount(null));
    expect(getDisplayPreferences().imageOrder).toBe('newest');
    expect(getDisplayPreferences().imageGrouping).toBe('filter');

    act(() => setPreferencesAccount('alice'));
    expect(getDisplayPreferences().imageGrouping).toBe('date');
    act(() => setPreferencesAccount(null));
    window.localStorage.clear();
  });

  it('reads a size or grouping it cannot use as the default', () => {
    act(() => setDisplayPreferences({
      ...base,
      gridThumbnailSize: 20,
      sequenceThumbnailSize: 9999,
      imageGrouping: 'project' as 'filter',
      imageGroupingAllProjects: 'session' as 'project',
    }));
    const preferences = getDisplayPreferences();
    expect(preferences.gridThumbnailSize).toBe(300);
    expect(preferences.sequenceThumbnailSize).toBe(150);
    expect(preferences.imageGrouping).toBe('filter');
    expect(preferences.imageGroupingAllProjects).toBe('project');
  });
});

describe('the Scoring control\'s earlier setting', () => {
  const LEGACY = 'psf-guard.penalty-scales';
  const OWN = 'psf-guard.display-preferences';

  /** The module as a fresh page load finds it. */
  async function load() {
    vi.resetModules();
    return import('../useDisplayPreferences');
  }

  beforeEach(() => window.localStorage.clear());

  it('becomes this browser\'s scoring defaults on the first load', async () => {
    window.localStorage.setItem(OWN, JSON.stringify({ libraryDensity: 'detailed' }));
    window.localStorage.setItem(LEGACY, JSON.stringify({
      satellite: 0, pointing: 1, temporal: 1.5, hfrRejectAbove: 2.55, starCountRejectBelow: null,
    }));
    const fresh = await load();
    expect(fresh.getDisplayPreferences()).toMatchObject({
      libraryDensity: 'detailed',
      scoringPenaltySatellite: 0,
      scoringPenaltyPointing: 1,
      scoringPenaltyTemporal: 1.5,
      scoringHfrRejectAbove: 2.55,
      scoringStarCountRejectBelow: null,
    });
    // Moved, not copied: the old key is gone and the browser's own
    // preferences hold the values, ready for a first sign-in.
    expect(window.localStorage.getItem(LEGACY)).toBeNull();
    expect(JSON.parse(window.localStorage.getItem(OWN)!)).toMatchObject({
      libraryDensity: 'detailed',
      scoringPenaltySatellite: 0,
      scoringHfrRejectAbove: 2.55,
    });
    // The first account to sign in here takes them with the rest.
    expect(fresh.claimBrowserPreferences()).toMatchObject({
      libraryDensity: 'detailed',
      scoringPenaltySatellite: 0,
      scoringHfrRejectAbove: 2.55,
    });
  });

  it('leaves defaults this browser already has alone', async () => {
    window.localStorage.setItem(OWN, JSON.stringify({ scoringHfrRejectAbove: 4 }));
    window.localStorage.setItem(LEGACY, JSON.stringify({ satellite: 0, hfrRejectAbove: 2.55 }));
    const fresh = await load();
    expect(fresh.getDisplayPreferences()).toMatchObject({
      scoringPenaltySatellite: 1,
      scoringHfrRejectAbove: 4,
    });
    expect(window.localStorage.getItem(LEGACY)).toBeNull();
  });

  it('with none, keeps the built-in defaults', async () => {
    const fresh = await load();
    expect(fresh.getDisplayPreferences()).toMatchObject(BUILT_IN_SCORING);
    expect(window.localStorage.getItem(OWN)).toBeNull();
  });

  it.each(['5', 'true', '"x"', '[1,2]', 'null'])(
    'loads, with every default, over a stored %s that is not an object',
    async (raw) => {
      window.localStorage.setItem(OWN, raw);
      window.localStorage.setItem(`${OWN}@alice`, raw);
      window.localStorage.setItem(LEGACY, JSON.stringify({ satellite: 0 }));
      const fresh = await load();
      expect(fresh.getDisplayPreferences()).toMatchObject({
        imageOrder: 'oldest',
      timeMarks: 'nights-hours',
        scoringPenaltySatellite: 0,
      });
      fresh.setPreferencesAccount('alice');
      expect(fresh.getDisplayPreferences()).toMatchObject({ scoringPenaltySatellite: 0 });
      fresh.setPreferencesAccount(null);
    },
  );

  it('drops an earlier value the rules refuse, keeping the rest', async () => {
    window.localStorage.setItem(LEGACY, JSON.stringify({
      satellite: 0.35, pointing: 0, starCountRejectBelow: 12.5, hfrRejectAbove: 250,
    }));
    const fresh = await load();
    expect(fresh.getDisplayPreferences()).toMatchObject({
      scoringPenaltySatellite: 1,
      scoringPenaltyPointing: 0,
      scoringHfrRejectAbove: null,
      scoringStarCountRejectBelow: null,
    });
  });

  it('offers its scoring defaults only to a document without any', async () => {
    window.localStorage.setItem(LEGACY, JSON.stringify({ starCountRejectBelow: 40 }));
    const fresh = await load();
    expect(fresh.claimBrowserScoring({ imageOrder: 'newest' })).toEqual({ ...BUILT_IN_SCORING, scoringStarCountRejectBelow: 40 });
    expect(fresh.claimBrowserScoring({ scoringPenaltySatellite: 1 })).toBeNull();
    // Once an account has claimed this browser's choices, nobody else gets them.
    fresh.retireBrowserPreferences();
    expect(fresh.claimBrowserScoring({ imageOrder: 'newest' })).toBeNull();
  });
});
