import { describe, expect, it } from 'vitest';
import { defaultColorProcessing } from '../stackColorProcessing';
import { defaultStretchRequest } from '../stackStretchModels';

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
  });

  it('does not alias stage objects between channel lanes', () => {
    const processing = defaultColorProcessing(['ha', 'oiii']);
    const ha = processing.input_stretches.ha![0];
    const oiii = processing.input_stretches.oiii![0];

    expect(ha).not.toBe(oiii);
    expect(ha.model).not.toBe(oiii.model);
  });
});
