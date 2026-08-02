import type { ArtifactSearchResult } from '../api/types';

export function morphologyLabel(result: ArtifactSearchResult): string | null {
  if (result.evidence === 'low') return null;
  switch (result.morphology) {
    case 'ring':
      return 'Ring / donut candidate';
    case 'broad_dark':
      return 'Dust-shadow candidate';
    case 'linear':
      return 'Trail-like candidate';
    case 'compact':
      return 'Compact spot';
    case 'diffuse':
      return 'Diffuse change';
    default:
      return 'Unclassified change';
  }
}

export function canBuildResidualFlat(
  result: ArtifactSearchResult,
  sourceKind: 'mono' | 'color'
): boolean {
  return sourceKind === 'mono'
    && result.evidence !== 'low'
    && result.direction === 'dark'
    && (result.morphology === 'ring' || result.morphology === 'broad_dark');
}
