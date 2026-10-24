import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type {
  ProcessingSetupsDocument,
  StackColorProcessing,
  StackColorRole,
} from '../../api/types';
import { defaultColorFor, fetchColorDefaults } from '../defaultColorSetup';
import { ALL_COLOR_ROLES, defaultColorProcessing } from '../stackColorProcessing';

// The server seeds the "Default" Color setup from this file.
const seeded = JSON.parse(
  readFileSync(resolve(__dirname, '../../../../src/processing_setups/default_color.json'), 'utf8')
) as StackColorProcessing;

function faint(): StackColorProcessing {
  return {
    ...structuredClone(seeded),
    background_extraction: null,
    input_stretches: Object.fromEntries(ALL_COLOR_ROLES.map((role) => [role, [{
      model: { type: 'auto-mtf', target_median: 0.1, shadows_clip: -2.8 },
      color_strategy: 'linked',
    }]])),
  };
}

function listing(processing?: StackColorProcessing): ProcessingSetupsDocument {
  return {
    schema_version: 1,
    setups: [],
    default_color: processing ? 'Faint' : null,
    ...(processing ? { default_color_processing: processing } : {}),
  };
}

describe('the default color pipeline', () => {
  it('is the pipeline the server seeds the Default setup with', () => {
    expect(defaultColorProcessing(ALL_COLOR_ROLES)).toEqual(seeded);
  });

  it('starts a new color build from the server default, narrowed to its channels', () => {
    const rgb = defaultColorFor(listing(faint()), ['red', 'green', 'blue']);
    expect(Object.keys(rgb.input_stretches)).toEqual(['red', 'green', 'blue']);
    expect(rgb.input_stretches.red?.[0].model).toMatchObject({ target_median: 0.1 });
    expect(rgb.background_extraction).toBeNull();
    expect(rgb.color_calibration).toEqual(seeded.color_calibration);

    // A narrowband palette is never color calibrated.
    const sho = defaultColorFor(listing(faint()), ['sii', 'ha', 'oiii']);
    expect(sho.color_calibration).toBeUndefined();
    expect(sho.input_stretches.ha?.[0].model).toMatchObject({ target_median: 0.1 });
  });

  it('is the built-in pipeline for each set of channels when the server marks none', () => {
    const channelSets: StackColorRole[][] = [
      ['red', 'green', 'blue'], ['ha', 'oiii'], ['luminance', 'red', 'green', 'blue'],
    ];
    for (const roles of channelSets) {
      expect(defaultColorFor(undefined, roles)).toEqual(defaultColorProcessing(roles));
      expect(defaultColorFor(listing(), roles)).toEqual(defaultColorProcessing(roles));
      // The seeded setup, as the server lists it, gives the same pipeline.
      expect(defaultColorFor(listing(seeded), roles)).toEqual(defaultColorProcessing(roles));
    }
  });

  it('sends the server default with a build, and the built-in one when the list cannot be read', async () => {
    server.use(http.get('/api/processing-setups', () => HttpResponse.json({
      success: true, data: listing(faint()), error: null,
    })));
    const client = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });
    expect(await fetchColorDefaults(client())).toEqual(faint());

    server.use(http.get('/api/processing-setups', () => HttpResponse.json(
      { success: false, data: null, error: 'broken' }, { status: 500 }
    )));
    expect(await fetchColorDefaults(client())).toEqual(defaultColorProcessing(ALL_COLOR_ROLES));
  });
});
