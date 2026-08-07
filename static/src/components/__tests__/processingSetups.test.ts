import { describe, expect, it } from 'vitest';
import type { StackColorProcessing } from '../../api/types';
import {
  builtinColorSetups,
  builtinViewSetups,
  colorSetupForRoles,
  setupExportFilename,
} from '../processingSetups';
import { defaultStretchRequest } from '../stackStretchModels';

function shoSetup(): StackColorProcessing {
  return {
    background_extraction: null,
    input_deconvolutions: {
      ha: { psf_fwhm_pixels: 3, iterations: 20, amount: 0.7,
            noise_fraction: 0.01, max_correction: 2 },
    },
    input_stretches: {
      ha: [defaultStretchRequest('auto-mtf')],
      oiii: [{ model: { type: 'mtf', shadows: 0, midtone: 0.4, highlights: 1 },
               color_strategy: 'linked' }],
      sii: [defaultStretchRequest('auto-mtf')],
    },
    output_stretches: [defaultStretchRequest('auto-mtf')],
  };
}

describe('colorSetupForRoles', () => {
  it('keeps matching channels and defaults the missing ones', () => {
    const mapped = colorSetupForRoles(shoSetup(), ['red', 'green', 'blue', 'ha']);
    // ha matches by name and keeps its stages and deconvolution.
    expect(mapped.input_stretches.ha).toEqual(shoSetup().input_stretches.ha);
    expect(mapped.input_deconvolutions.ha).toBeDefined();
    // The setup treats its channels differently, so foreign channels get the
    // standing default rather than a guessed mapping.
    expect(mapped.input_stretches.red).toEqual([defaultStretchRequest('auto-mtf')]);
    expect(mapped.input_deconvolutions.red).toBeUndefined();
    // Channels absent from the target card do not come along.
    expect(mapped.input_stretches.oiii).toBeUndefined();
  });

  it('carries one uniform channel treatment to every target channel', () => {
    const uniform = shoSetup();
    const stages = [{
      model: { type: 'mtf' as const, shadows: 0.01, midtone: 0.35, highlights: 1 },
      color_strategy: 'linked' as const,
    }];
    uniform.input_stretches = { ha: stages, oiii: stages, sii: stages };
    const mapped = colorSetupForRoles(uniform, ['red', 'green', 'blue']);
    expect(mapped.input_stretches.red).toEqual(stages);
    expect(mapped.input_stretches.green).toEqual(stages);
    expect(mapped.input_stretches.blue).toEqual(stages);
  });

  it('passes the pipeline-wide settings through untouched', () => {
    const setup = shoSetup();
    const mapped = colorSetupForRoles(setup, ['ha', 'oiii', 'sii']);
    expect(mapped.background_extraction).toBeNull();
    expect(mapped.output_stretches).toEqual(setup.output_stretches);
  });

  it('tolerates a setup with no role-keyed fields at all', () => {
    const empty = {
      background_extraction: null,
      input_deconvolutions: {},
      input_stretches: {},
      output_stretches: [],
    } as StackColorProcessing;
    const mapped = colorSetupForRoles(empty, ['red', 'green', 'blue']);
    expect(mapped.input_stretches.red).toEqual([defaultStretchRequest('auto-mtf')]);
    expect(Object.keys(mapped.input_deconvolutions)).toHaveLength(0);
  });
});

describe('built-in setups', () => {
  it('offers only the identity view on a display-referred image', () => {
    const setups = builtinViewSetups(true);
    expect(setups).toHaveLength(1);
    expect(setups[0].settings.model.type).toBe('identity');
  });

  it('derives color built-ins from the card roles', () => {
    const rgb = builtinColorSetups[0].settings(['red', 'green', 'blue']);
    expect(Object.keys(rgb.input_stretches).sort()).toEqual(['blue', 'green', 'red']);
    const bare = builtinColorSetups[1].settings(['ha', 'oiii']);
    expect(bare.background_extraction).toBeNull();
  });
});

describe('setupExportFilename', () => {
  it('slugs a name into a safe download filename', () => {
    expect(setupExportFilename('Deep SHO pipeline')).toBe('psf-guard-setup-deep-sho-pipeline.json');
    expect(setupExportFilename('Gentle Hα view')).toBe('psf-guard-setup-gentle-h-view.json');
    expect(setupExportFilename('···')).toBe('psf-guard-setup-unnamed.json');
  });
});
