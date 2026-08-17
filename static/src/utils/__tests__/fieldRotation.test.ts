import { describe, expect, it } from 'vitest';
import { fieldRotation } from '../fieldRotation';
import type { WcsSolution } from '@seiza/astro-overlay';

function wcs(cd: [[number, number], [number, number]]): WcsSolution {
  return { crval: [180, 45], crpix: [100, 100], cd };
}

const SCALE = 0.001; // deg/px

describe('fieldRotation', () => {
  it('reports zero for north-up east-left', () => {
    // +Dec maps to −y (screen up), +RA to −x (screen left).
    const rotation = fieldRotation(wcs([[-SCALE, 0], [0, -SCALE]]))!;
    expect(rotation.degrees).toBeCloseTo(0, 6);
    expect(rotation.mirrored).toBe(false);
  });

  it('reports a quarter turn when north points right', () => {
    // +Dec maps to +x: the camera is rotated 90°.
    const rotation = fieldRotation(wcs([[0, -SCALE], [SCALE, 0]]))!;
    expect(rotation.degrees).toBeCloseTo(90, 6);
    expect(rotation.mirrored).toBe(false);
  });

  it('flags a mirrored field', () => {
    // North up but east to the RIGHT: parity flipped.
    const rotation = fieldRotation(wcs([[SCALE, 0], [0, -SCALE]]))!;
    expect(rotation.degrees).toBeCloseTo(0, 6);
    expect(rotation.mirrored).toBe(true);
  });

  it('returns null for a degenerate matrix', () => {
    expect(fieldRotation(wcs([[0, 0], [0, 0]]))).toBeNull();
  });
});
