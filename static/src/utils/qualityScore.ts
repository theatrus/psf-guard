import type { ImageQualityResult } from '../api/types';

export type QualityScoreScope = 'capture_sequence' | 'target_filter';

export function hasPixelQualityEvidence(result: ImageQualityResult): boolean {
  const metrics = result.normalized_metrics;
  return metrics.spatial_coverage != null
    || metrics.transparency != null
    || metrics.pointing != null;
}

export function qualityScoreBasis(result: ImageQualityResult): string {
  return hasPixelQualityEvidence(result) ? 'Pixel-assisted score' : 'Catalog-relative score';
}

export function qualityScoreDescription(
  result: ImageQualityResult,
  scope: QualityScoreScope,
): string {
  const score = `${Math.round(result.quality_score * 100)}%`;
  const basis = qualityScoreBasis(result);
  const comparison = scope === 'capture_sequence'
    ? 'the same capture session'
    : 'all target/filter stack candidates with matching capture settings';
  const evidence = hasPixelQualityEvidence(result)
    ? 'It includes fresh pixel evidence when available.'
    : 'It uses catalog metrics only and is not proof of image damage.';
  return `${basis}: ${score}. Compared with ${comparison}. ${evidence}`;
}

/** Tooltip for the smaller second badge showing the other comparison basis. */
export function secondaryScoreDescription(score: number, scope: QualityScoreScope): string {
  const value = `${Math.round(score * 100)}%`;
  return scope === 'capture_sequence'
    ? `Within its own capture session: ${value}. A small session can flatter a frame; the main badge compares across every session of this filter.`
    : `Across all target/filter stack candidates: ${value}. The main badge compares within the selected capture session.`;
}
