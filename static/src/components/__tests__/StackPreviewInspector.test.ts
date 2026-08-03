import { describe, expect, it } from 'vitest';
import type { ArtifactSearchResult } from '../../api/types';
import { morphologyLabel } from '../artifactMorphology';
import { artifactRegionFromPoints, isArtifactRegionCapped } from '../stackArtifactRegion';

function result(
  morphology: ArtifactSearchResult['morphology'],
  evidence: ArtifactSearchResult['evidence'] = 'strong'
): ArtifactSearchResult {
  return {
    image_id: 1,
    filter_name: 'Ha',
    acquired_unix_seconds: null,
    grading_status: 0,
    score: 12,
    peak_sigma: 10,
    bright_fraction: 0,
    dark_fraction: 0.1,
    coverage_fraction: 1,
    evidence,
    direction: 'dark',
    morphology,
    crop_url: '/crop.png',
  };
}

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

  it('rejects a drag that is still too small to search', () => {
    expect(artifactRegionFromPoints(
      { x: 0, y: 0 },
      { x: 7, y: 20 },
      100,
      100
    )).toBeNull();
  });

  it('stops a long drag at the limit instead of voiding the box', () => {
    // Dragging past the limit used to return null, so the box vanished
    // mid-drag and the selection had to be started over.
    expect(artifactRegionFromPoints(
      { x: 100, y: 100 },
      { x: 900, y: 900 },
      2000,
      2000
    )).toEqual({ x: 100, y: 100, width: 512, height: 512 });
  });

  it('keeps the corner the drag began at when it clamps backwards', () => {
    // Dragging up and left, the anchor is the bottom-right corner, so that is
    // the edge that has to stay put.
    expect(artifactRegionFromPoints(
      { x: 600, y: 700 },
      { x: 0, y: 0 },
      2000,
      2000
    )).toEqual({ x: 88, y: 188, width: 512, height: 512 });
  });

  it('clamps each side on its own', () => {
    expect(artifactRegionFromPoints(
      { x: 0, y: 0 },
      { x: 900, y: 40 },
      2000,
      2000
    )).toEqual({ x: 0, y: 0, width: 512, height: 40 });
  });

  it('clamps a drag that runs off the image to the limit, not the edge', () => {
    expect(artifactRegionFromPoints(
      { x: 50, y: 50 },
      { x: 5000, y: 5000 },
      1000,
      1000
    )).toEqual({ x: 50, y: 50, width: 512, height: 512 });
  });

  it('reports when a region has grown to the limit', () => {
    expect(isArtifactRegionCapped({ x: 0, y: 0, width: 512, height: 40 })).toBe(true);
    expect(isArtifactRegionCapped({ x: 0, y: 0, width: 40, height: 512 })).toBe(true);
    expect(isArtifactRegionCapped({ x: 0, y: 0, width: 511, height: 511 })).toBe(false);
  });
});

describe('morphologyLabel', () => {
  it('uses cautious labels for dust shadows and rings', () => {
    expect(morphologyLabel(result('broad_dark'))).toBe('Dust-shadow candidate');
    expect(morphologyLabel(result('ring'))).toBe('Ring / donut candidate');
  });

  it('does not label low-evidence shape guesses', () => {
    expect(morphologyLabel(result('ring', 'low'))).toBeNull();
  });
});
