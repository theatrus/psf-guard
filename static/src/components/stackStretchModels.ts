import type { StackStretchModel, StackStretchRequest } from '../api/types';

export type StretchModelType = StackStretchModel['type'];

export const stretchModelLabels: Record<StretchModelType, string> = {
  identity: 'Identity',
  linear: 'Linear range',
  asinh: 'Asinh',
  'percentile-asinh': 'Percentile asinh',
  mtf: 'MTF',
  ghs: 'Generalized hyperbolic',
  'auto-mtf': 'Auto MTF',
};

export function defaultStretchModel(type: StretchModelType): StackStretchModel {
  switch (type) {
    case 'identity': return { type };
    case 'linear': return { type, black: 0, white: 1 };
    case 'asinh': return { type, black: 0, white: 1, strength: 10 };
    case 'percentile-asinh':
      return { type, black_percentile: 0.01, white_percentile: 0.995, strength: 10 };
    case 'mtf': return { type, shadows: 0, midtone: 0.5, highlights: 1 };
    case 'ghs':
      return {
        type,
        stretch_factor: 1,
        local_intensity: 0,
        symmetry_point: 0,
        protect_shadows: 0,
        protect_highlights: 1,
        black: 0,
        white: 1,
      };
    case 'auto-mtf': return { type, target_median: 0.2, shadows_clip: -2.8 };
  }
}

export function defaultStretchRequest(
  type: StretchModelType = 'auto-mtf'
): StackStretchRequest {
  return {
    model: defaultStretchModel(type),
    color_strategy: 'linked',
  };
}
