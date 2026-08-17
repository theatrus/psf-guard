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

/** Every comparison basis a frame is scored under. `all` exists only when
 * the target/filter has more than one session to compare across. */
export interface BasisScores {
  night: number;
  all?: number;
}

/** Tooltip for a basis chip. The chips render on every card so the same
 * frame never gains or loses one between views. */
export function basisChipDescription(score: number, scope: QualityScoreScope): string {
  const value = `${Math.round(score * 100)}%`;
  return scope === 'capture_sequence'
    ? `Night session score: ${value}, compared within the frame's own capture session. A small session can flatter a frame.`
    : `All-sessions score: ${value}, compared across every stack candidate for this filter.`;
}
