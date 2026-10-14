import { beforeEach, describe, expect, it } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import {
  getDisplayPreferences,
  setDetailSection,
  setDisplayPreferences,
  setPreferencesAccount,
  useDisplayPreferences,
} from '../useDisplayPreferences';

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
      imageGrouping: 'filter',
      imageGroupingAllProjects: 'project',
      gridThumbnailSize: 300,
      sequenceThumbnailSize: 150,
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
      imageGrouping: 'filter',
      imageGroupingAllProjects: 'project',
      gridThumbnailSize: 300,
      sequenceThumbnailSize: 150,
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
