import type {
  StackBackgroundConfig,
  StackBackgroundExtraction,
  StackColorCalibration,
  StackColorCombination,
  StackColorJob,
  StackColorProcessing,
  StackColorRole,
} from '../api/types';
import { defaultStretchRequest } from './stackStretchModels';

export const BACKGROUND_COLOR_CACHE_VERSION = 4;

/** Every channel a color preview can use, in display order. */
export const ALL_COLOR_ROLES: StackColorRole[] = [
  'luminance', 'red', 'green', 'blue', 'ha', 'oiii', 'sii',
];

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

/** The built-in color pipeline, for these channels. Its inputs run the
 *  deconvolution and RC-Astro steps their channel cards applied. The server
 *  seeds the "Default" Color setup with this pipeline for every channel
 *  (src/processing_setups/default_color.json, checked by a test), and new
 *  color builds start from whichever setup is marked default; this is the
 *  fallback when the server's choice is not available. */
export function defaultColorProcessing(roles: StackColorRole[]): StackColorProcessing {
  return {
    input_processing: 'channels',
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

/** Explicit starting recipe for a new automatic narrowband card. Existing
 *  artifacts keep their saved pipeline. Preserve custom per-filter stretches. */
export function automaticNarrowbandProcessing(processing: StackColorProcessing): StackColorProcessing {
  if (processing.narrowband) return processing;
  const stages = ['ha', 'oiii', 'sii'].flatMap((role) => {
    const value = processing.input_stretches[role as StackColorRole];
    return value ? [JSON.stringify(value)] : [];
  });
  return {
    ...processing,
    narrowband: {
      shared_stretch: stages.every((value) => value === stages[0]),
      balance: { has_sii: true, shadow_protection: 1, oiii_boost: 1, sii_boost: 1, max_samples: 1_000_000 },
    },
  };
}

/** Drop tool settings for inputs that a changed star choice no longer uses. */
export function retainColorInputRoles(processing: StackColorProcessing, roles: StackColorRole[]): StackColorProcessing {
  const keep = <T,>(entries: Partial<Record<StackColorRole, T>>) => Object.fromEntries(
    Object.entries(entries).filter(([role]) => roles.includes(role as StackColorRole))
  );
  return {
    ...processing,
    input_stretches: keep(processing.input_stretches),
    input_deconvolutions: keep(processing.input_deconvolutions ?? {}),
    input_rc_astro: keep(processing.input_rc_astro ?? {}),
  };
}

/** What a color build of these channels runs: the preview's own processing,
 *  or, for a new or outdated preview, `defaults` for its channels. */
export function processingForColorBuild(
  artifact: Pick<StackColorJob, 'cache_version' | 'processing'> | undefined,
  roles: StackColorRole[],
  defaults: (roles: StackColorRole[]) => StackColorProcessing = defaultColorProcessing
): StackColorProcessing {
  if (!artifact || artifact.cache_version < BACKGROUND_COLOR_CACHE_VERSION) {
    return defaults(roles);
  }
  const processing = artifact.processing ?? defaults(roles);
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

/** Combined recipes opt into continuum processing; ordinary saved recipes keep their settings. */
export function combinedColorProcessing(processing: StackColorProcessing, combination: StackColorCombination | undefined, roles: StackColorRole[]): StackColorProcessing {
  if (!combination) {
    const next = { ...processing }; delete next.emission; return next;
  }
  const lines = Object.fromEntries(roles.filter((r) => ['ha', 'oiii', 'sii'].includes(r)).map((role) => [role,
    processing.emission?.lines[role] ?? { amount: 0.25, coefficients: [], reference_flux_ratios: [] }]));
  return { ...processing, emission: { mode: 'automatic', positive_contribution: true, broadband_amount: 0,
    ...processing.emission, lines } };
}
