import { describe, expect, it } from 'vitest';
import type { StackMethod } from '../../api/types';
import { sameMethod } from '../stackMethod';

const auto: StackMethod = {
  normalization: 'local_background',
  weighting: 'noise',
  registration: 'quadratic',
  interpolation: 'lanczos3',
  bayer_drizzle: false,
  reference: 'auto',
  final_pass: 'reintegrate',
  reference_rotation_margin_degrees: 1,
};

describe('sameMethod', () => {
  it('counts the rotation margin only when Seiza chooses the reference', () => {
    expect(sameMethod(auto, { ...auto, reference_rotation_margin_degrees: 2.5 })).toBe(false);
    const graded: StackMethod = { ...auto, reference: 'best_graded' };
    // A stack built under Best graded is not out of date for a margin it never used.
    expect(sameMethod(graded, { ...graded, reference_rotation_margin_degrees: 2.5 })).toBe(true);
    expect(sameMethod(graded, { ...auto, reference_rotation_margin_degrees: 2.5 })).toBe(false);
  });
});
