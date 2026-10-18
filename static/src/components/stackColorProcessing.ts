import type {
  StackBackgroundConfig,
  StackBackgroundExtraction,
  StackColorCalibration,
  StackColorJob,
  StackColorProcessing,
  StackColorRole,
} from '../api/types';
import { defaultStretchRequest } from './stackStretchModels';

export const BACKGROUND_COLOR_CACHE_VERSION = 4;

export function defaultBackgroundModel(
  kind: StackBackgroundConfig['model']['kind'] = 'automatic'
): StackBackgroundConfig['model'] {
  if (kind === 'polynomial') return { kind, degree: 2, ridge: 1e-8 };
  if (kind === 'radial_basis') return { kind, smoothing: 0.01, max_control_points: 192 };
  return {
    kind,
    max_degree: 2,
    ridge: 1e-8,
    rbf_smoothing: 0.01,
    max_control_points: 192,
    allow_radial_basis: false,
    minimum_improvement: 0.08,
  };
}

export function defaultBackgroundExtraction(): StackBackgroundExtraction {
  return {
    config: {
      model: defaultBackgroundModel(),
      samples_per_axis: 12,
      sample_radius: null,
      search_steps: 4,
      sample_rejection_sigma: 3.5,
      fit_rejection_sigma: 3,
      fit_rejection_iterations: 3,
      border_fraction: 0.03,
    },
    correction_mode: 'subtract',
    strength: 1,
    protect_catalog_emission: true,
  };
}

export function defaultColorCalibration(): StackColorCalibration {
  return { enabled: true, neutralize_background: true, white_reference: 'solar' };
}

/** Whether a composition of these channels can be color calibrated: RGB and
 *  LRGB can, false-color narrowband palettes cannot. */
export function colorCalibrationApplies(roles: StackColorRole[]): boolean {
  return (['red', 'green', 'blue'] as const).every((role) => roles.includes(role));
}

export function defaultColorProcessing(roles: StackColorRole[]): StackColorProcessing {
  return {
    background_extraction: defaultBackgroundExtraction(),
    input_deconvolutions: {},
    input_rc_astro: {},
    input_stretches: Object.fromEntries(
      roles.map((role) => [role, [defaultStretchRequest('auto-mtf')]])
    ),
    output_stretches: [],
    ...(colorCalibrationApplies(roles) ? { color_calibration: defaultColorCalibration() } : {}),
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
    background_extraction: processing.background_extraction
      ? {
          ...processing.background_extraction,
          strength: processing.background_extraction.strength ?? 1,
          protect_catalog_emission:
            processing.background_extraction.protect_catalog_emission ?? true,
        }
      : null,
    input_deconvolutions: processing.input_deconvolutions ?? {},
    input_rc_astro: processing.input_rc_astro ?? {},
  };
}
