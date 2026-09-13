import { describe, expect, it } from 'vitest';
import type { ImageQualityResult } from '../../api/types';
import {
  availableFlags,
  flagFilterLabel,
  matchesFlagFilter,
  parseFlagFilter,
} from '../flagFilter';

const result = (flags: string[]): ImageQualityResult =>
  ({ image_id: 1, quality_score: 0.5, category: null, flags } as unknown as ImageQualityResult);

describe('flagFilter', () => {
  it('treats an empty or all value as no filter', () => {
    expect(parseFlagFilter(null)).toBe('all');
    expect(parseFlagFilter(' All ')).toBe('all');
    expect(parseFlagFilter('Rotation_Skew')).toBe('rotation_skew');
    expect(matchesFlagFilter('all', undefined)).toBe(true);
    expect(matchesFlagFilter('', result([]))).toBe(true);
  });

  it('keeps only images whose quality result carries the flag', () => {
    expect(matchesFlagFilter('rotation_skew', result(['rotation_skew', 'off_target']))).toBe(true);
    expect(matchesFlagFilter('rotation_skew', result(['off_target']))).toBe(false);
    expect(matchesFlagFilter('rotation_skew', undefined)).toBe(false);
  });

  it('lists the flags present, labelled the way the badges are', () => {
    const flags = availableFlags([result(['rotation_skew']), result(['off_target', 'rotation_skew'])]);
    expect(flags).toEqual(['off_target', 'rotation_skew']);
    expect(flagFilterLabel('rotation_skew')).toBe('Rotation Skew');
    expect(flagFilterLabel('all')).toBe('All');
  });
});
