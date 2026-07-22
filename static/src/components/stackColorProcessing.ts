import type {
  StackBackgroundExtraction,
  StackColorJob,
  StackColorProcessing,
  StackColorRole,
} from '../api/types';
import { defaultStretchRequest } from './stackStretchModels';

export const BACKGROUND_COLOR_CACHE_VERSION = 4;

export function defaultBackgroundExtraction(): StackBackgroundExtraction {
  return {
    config: {
      model: { kind: 'polynomial', degree: 2, ridge: 1e-8 },
      samples_per_axis: 12,
      sample_radius: null,
      search_steps: 4,
      sample_rejection_sigma: 3.5,
      fit_rejection_sigma: 3,
      fit_rejection_iterations: 3,
      border_fraction: 0.03,
    },
    correction_mode: 'subtract',
  };
}

export function defaultColorProcessing(roles: StackColorRole[]): StackColorProcessing {
  return {
    background_extraction: defaultBackgroundExtraction(),
    input_deconvolutions: {},
    input_stretches: Object.fromEntries(
      roles.map((role) => [role, [defaultStretchRequest('auto-mtf')]])
    ),
    output_stretches: [],
  };
}

export function processingForColorBuild(
  artifact: Pick<StackColorJob, 'cache_version' | 'processing'> | undefined,
  roles: StackColorRole[]
): StackColorProcessing {
  if (!artifact || artifact.cache_version < BACKGROUND_COLOR_CACHE_VERSION) {
    return defaultColorProcessing(roles);
  }
  const processing = artifact.processing ?? defaultColorProcessing(roles);
  return {
    ...processing,
    input_deconvolutions: processing.input_deconvolutions ?? {},
  };
}
