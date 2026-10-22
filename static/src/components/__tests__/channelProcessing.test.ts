import { describe, expect, it } from 'vitest';
import type { StackStretchPreview } from '../../api/types';
import {
  appliedProcessingQueryKey,
  channelProcessingFrom,
  channelToolsLabel,
  ownFromChannels,
} from '../channelProcessing';
import { defaultColorProcessing } from '../stackColorProcessing';
import { defaultDeconvolutionConfig } from '../stackDeconvolution';

function preview(request: StackStretchPreview['request']): StackStretchPreview {
  return { request } as StackStretchPreview;
}

describe('channel processing', () => {
  it("takes each channel card's deconvolution and RC-Astro steps, and nothing else", () => {
    const deconvolution = defaultDeconvolutionConfig();
    const channels = channelProcessingFrom(
      [{ role: 'ha' }, { role: 'oiii' }, { role: 'sii' }, { role: 'red' }],
      [
        preview({ rc_astro: { steps: [{ tool: 'bxt', parameters: { sharpen_stars: 0.3 } }] } } as never),
        preview({ deconvolution, rc_astro: { steps: [] } } as never),
        // A card showing only a stretch has nothing to copy.
        preview({} as never),
        // Nor does a card whose processing has not loaded.
        undefined,
      ]
    );
    expect(channels).toEqual({
      ha: { rc_astro: { steps: [{ tool: 'bxt', parameters: { sharpen_stars: 0.3 } }] } },
      oiii: { deconvolution },
    });
  });

  it('starts own settings from exactly what the cards show', () => {
    const deconvolution = defaultDeconvolutionConfig();
    const draft = {
      ...defaultColorProcessing(['red', 'green', 'blue']),
      // Left from an earlier build: Blue's card shows nothing now.
      input_deconvolutions: { blue: deconvolution },
    };
    const own = ownFromChannels(draft, {
      red: { deconvolution },
      green: { rc_astro: { steps: [{ tool: 'nxt', parameters: {} }] } },
    }, ['red', 'green', 'blue']);
    expect(own.input_processing).toBe('own');
    expect(own.input_deconvolutions).toEqual({ red: deconvolution });
    expect(own.input_rc_astro).toEqual({ green: { steps: [{ tool: 'nxt', parameters: {} }] } });
    expect(own.input_stretches).toEqual(draft.input_stretches);
  });

  it("names a card's tools in the order they run", () => {
    const name = (tool: string) => ({ bxt: 'BlurXTerminator', nxt: 'NoiseXTerminator' })[tool] ?? tool;
    expect(channelToolsLabel({
      deconvolution: defaultDeconvolutionConfig(),
      rc_astro: { steps: [{ tool: 'bxt', parameters: {} }, { tool: 'nxt', parameters: {} }] },
    }, name)).toBe('deconvolution, BlurXTerminator, NoiseXTerminator');
    expect(channelToolsLabel({}, name)).toBeNull();
    expect(channelToolsLabel(undefined, name)).toBeNull();
  });

  it('shares the stacks panel query for one exact channel revision', () => {
    expect(appliedProcessingQueryKey('db-a', 3, 'job', 1, 'rev')).toEqual(
      ['db', 'db-a', 'project', 3, 'stack-processing', 'job:1:rev']
    );
  });
});
