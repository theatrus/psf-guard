import { describe, expect, it } from 'vitest';
import {
  BACKGROUND_COLOR_CACHE_VERSION,
  defaultColorProcessing,
  processingForColorBuild,
} from '../stackColorProcessing';
import { defaultStretchRequest } from '../stackStretchModels';
import {
  defaultDeconvolutionConfig,
  validateDeconvolution,
} from '../stackDeconvolution';

describe('stack color processing defaults', () => {
  it('starts every physical input with its own Auto-MTF stage', () => {
    const processing = defaultColorProcessing(['red', 'green', 'blue']);

    expect(processing.input_stretches.red).toEqual([
      defaultStretchRequest('auto-mtf'),
    ]);
    expect(processing.input_stretches.green).toEqual([
      defaultStretchRequest('auto-mtf'),
    ]);
    expect(processing.input_stretches.blue).toEqual([
      defaultStretchRequest('auto-mtf'),
    ]);
    expect(processing.output_stretches).toEqual([]);
    expect(processing.input_deconvolutions).toEqual({});
    expect(processing.background_extraction?.correction_mode).toBe('subtract');
    expect(processing.background_extraction?.config.model.degree).toBe(2);
  });

  it('does not alias stage objects between channel lanes', () => {
    const processing = defaultColorProcessing(['ha', 'oiii']);
    const ha = processing.input_stretches.ha![0];
    const oiii = processing.input_stretches.oiii![0];

    expect(ha).not.toBe(oiii);
    expect(ha.model).not.toBe(oiii.model);
  });

  it('migrates version-invalidated artifacts to the background-enabled defaults', () => {
    const legacy = defaultColorProcessing(['red', 'green', 'blue']);
    legacy.background_extraction = null;

    const migrated = processingForColorBuild({
      cache_version: BACKGROUND_COLOR_CACHE_VERSION - 1,
      processing: legacy,
    }, ['red', 'green', 'blue']);

    expect(migrated.background_extraction).not.toBeNull();
  });

  it('preserves an explicit background opt-out on current artifacts', () => {
    const processing = defaultColorProcessing(['red', 'green', 'blue']);
    processing.background_extraction = null;

    expect(processingForColorBuild({
      cache_version: BACKGROUND_COLOR_CACHE_VERSION,
      processing,
    }, ['red', 'green', 'blue']).background_extraction).toBeNull();
  });

  it('migrates current artifacts without deconvolution to the disabled state', () => {
    const processing = defaultColorProcessing(['red', 'green', 'blue']);
    const legacy = { ...processing } as Partial<typeof processing>;
    delete legacy.input_deconvolutions;

    expect(processingForColorBuild({
      cache_version: BACKGROUND_COLOR_CACHE_VERSION,
      processing: legacy as typeof processing,
    }, ['red', 'green', 'blue']).input_deconvolutions).toEqual({});
  });

  it('uses conservative upstream defaults only after deconvolution is enabled', () => {
    const processing = defaultColorProcessing(['ha', 'oiii']);
    expect(processing.input_deconvolutions.ha).toBeUndefined();

    const config = defaultDeconvolutionConfig();
    expect(validateDeconvolution(config)).toBeNull();
    expect(config).toEqual({
      psf_fwhm_pixels: 3.1,
      iterations: 4,
      amount: 0.35,
      noise_fraction: 0.001,
      max_correction: 2,
    });
  });
});
