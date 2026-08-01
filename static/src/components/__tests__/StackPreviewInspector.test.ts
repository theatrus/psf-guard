import { describe, expect, it } from 'vitest';
import { artifactRegionFromPoints } from '../stackArtifactRegion';

describe('artifactRegionFromPoints', () => {
  it('maps a reverse drag to integer image coordinates', () => {
    expect(artifactRegionFromPoints(
      { x: 120.8, y: 80.2 },
      { x: 20.4, y: 10.7 },
      200,
      100
    )).toEqual({ x: 20, y: 10, width: 101, height: 71 });
  });

  it('clips the region to the image bounds', () => {
    expect(artifactRegionFromPoints(
      { x: -10, y: 90 },
      { x: 20, y: 110 },
      100,
      100
    )).toEqual({ x: 0, y: 90, width: 20, height: 10 });
  });

  it('rejects regions that are too small or too large', () => {
    expect(artifactRegionFromPoints(
      { x: 0, y: 0 },
      { x: 7, y: 20 },
      100,
      100
    )).toBeNull();
    expect(artifactRegionFromPoints(
      { x: 0, y: 0 },
      { x: 513, y: 20 },
      1000,
      1000
    )).toBeNull();
  });
});
