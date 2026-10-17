import { describe, expect, it } from 'vitest';
import type { StackStretchPreview } from '../../api/types';
import { appliedProcessingQueryKey, channelProcessingFrom } from '../channelProcessing';
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

  it('shares the stacks panel query for one exact channel revision', () => {
    expect(appliedProcessingQueryKey('db-a', 3, 'job', 1, 'rev')).toEqual(
      ['db', 'db-a', 'project', 3, 'stack-processing', 'job:1:rev']
    );
  });
});
