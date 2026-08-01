import { describe, expect, it } from 'vitest';
import type { ArtifactSearchResult } from '../../api/types';
import { morphologyLabel } from '../artifactMorphology';
import { artifactRegionFromPoints } from '../stackArtifactRegion';

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

describe('morphologyLabel', () => {
  it('uses cautious labels for dust shadows and rings', () => {
    expect(morphologyLabel(result('broad_dark'))).toBe('Dust-shadow candidate');
    expect(morphologyLabel(result('ring'))).toBe('Ring / donut candidate');
  });

  it('does not label low-evidence shape guesses', () => {
    expect(morphologyLabel(result('ring', 'low'))).toBeNull();
  });
});
