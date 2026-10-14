import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  calibrationSectionOpen,
  useCalibrationSection,
  useDetailSection,
} from '../useDetailSections';
import { setDisplayPreferences } from '../useDisplayPreferences';

function resetPreferences() {
  window.localStorage.removeItem('psf-guard.display-preferences');
  setDisplayPreferences({
    showNightChip: true,
    showAllChip: true,
    showCalibrationChip: true,
    advanceOnGrade: true,
    projectPickerGrouping: 'activity',
    libraryDensity: 'compact',
    detailSections: {},
  });
}

describe('calibrationSectionOpen', () => {
  it.each([
    // choice, marked, forced, open
    [undefined, true, false, true],
    [undefined, false, false, false],
    ['open', false, false, true],
    ['closed', true, false, false],
    [undefined, false, true, true],
    ['closed', false, true, true],
  ] as const)('choice %s, marked %s, forced %s: open %s', (choice, marked, forced, open) => {
    expect(calibrationSectionOpen(choice, marked, forced)).toBe(open);
  });
});

describe('useDetailSection', () => {
  beforeEach(resetPreferences);

  it('is open until folded, and every view of it agrees, across images and reloads', () => {
    const { result, rerender } = renderHook(({ imageId }) => {
      void imageId;
      return useDetailSection('sky');
    }, { initialProps: { imageId: 8 } });
    expect(result.current.open).toBe(true);

    act(() => result.current.toggle());
    expect(result.current.open).toBe(false);
    rerender({ imageId: 9 });
    expect(result.current.open).toBe(false);
    expect(
      JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!).detailSections,
    ).toEqual({ sky: 'closed' });

    // Another section is its own choice.
    const quality = renderHook(() => useDetailSection('quality'));
    expect(quality.result.current.open).toBe(true);
  });
});

describe('useCalibrationSection', () => {
  beforeEach(resetPreferences);

  it('follows the mark until the viewer chooses, then keeps the choice', () => {
    const { result, rerender } = renderHook(
      ({ marked }) => useCalibrationSection(marked, ''),
      { initialProps: { marked: true } },
    );
    expect(result.current.open).toBe(true);
    rerender({ marked: false });
    expect(result.current.open).toBe(false);

    act(() => result.current.toggle());
    expect(result.current.open).toBe(true);
    rerender({ marked: false });
    expect(result.current.open).toBe(true);

    act(() => result.current.toggle());
    rerender({ marked: true });
    expect(result.current.open).toBe(false);
  });

  it('opens for a purpose, and a fold holds until the purpose changes', () => {
    const { result, rerender } = renderHook(
      ({ purpose }) => useCalibrationSection(false, purpose),
      { initialProps: { purpose: 'thin_dark' } },
    );
    expect(result.current.open).toBe(true);

    act(() => result.current.toggle());
    expect(result.current.open).toBe(false);
    rerender({ purpose: 'thin_dark' });
    expect(result.current.open).toBe(false);

    rerender({ purpose: 'mixed_flats' });
    expect(result.current.open).toBe(true);
    // No purpose: the stored "closed" is the rule again.
    rerender({ purpose: '' });
    expect(result.current.open).toBe(false);
  });
});
