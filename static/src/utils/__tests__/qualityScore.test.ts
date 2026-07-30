import { describe, expect, it } from 'vitest';
import type { ImageQualityResult } from '../../api/types';
import {
  hasPixelQualityEvidence,
  qualityScoreBasis,
  qualityScoreDescription,
} from '../qualityScore';

function result(overrides: Partial<ImageQualityResult> = {}): ImageQualityResult {
  return {
    image_id: 1,
    quality_score: 0.42,
    temporal_anomaly_score: 0.01,
    category: null,
    normalized_metrics: {
      star_count: 0.4,
      hfr: 0.6,
      eccentricity: null,
      snr: null,
      background: null,
    },
    details: null,
    ...overrides,
  };
}

describe('quality score labels', () => {
  it('calls catalog-only scores relative evidence, not damage', () => {
    const quality = result();
    expect(hasPixelQualityEvidence(quality)).toBe(false);
    expect(qualityScoreBasis(quality)).toBe('Catalog-relative score');
    expect(qualityScoreDescription(quality, 'capture_sequence')).toContain(
      'not proof of image damage',
    );
  });

  it('describes target/filter comparisons and fresh pixel evidence', () => {
    const quality = result({
      normalized_metrics: {
        ...result().normalized_metrics,
        spatial_coverage: 0.8,
      },
    });
    expect(hasPixelQualityEvidence(quality)).toBe(true);
    expect(qualityScoreBasis(quality)).toBe('Pixel-assisted score');
    expect(qualityScoreDescription(quality, 'target_filter')).toContain(
      'all target/filter stack candidates with matching capture settings',
    );
  });
});
