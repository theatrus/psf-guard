import { describe, expect, it } from 'vitest';
import type { ExternalToolSchema } from '../../api/types';
import { rcAstroProblem } from '../rcAstroSchema';

const bxt: ExternalToolSchema = {
  schema_version: 6, cli_version: '2.6.6', key: 'bxt', name: 'RC-Astro BlurXTerminator',
  ml_version: 5, licensed: true, license_message: null,
  parameters: [
    { name: 'ss', flag: '--ss', label: 'Sharpen Stars', description: '',
      kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } },
    { name: 'sn', flag: '--sn', label: 'Sharpen Nonstellar', description: '',
      kind: { type: 'float', default: 0.5, min: 0, max: 1 } },
    { name: 'tiles', flag: '--tiles', label: 'Tiles', description: '',
      kind: { type: 'int', default: 2, min: 1, max: 8 } },
    { name: 'lp', flag: '--lp', label: 'Lunar/Planetary Mode', description: '',
      kind: { type: 'bool', default: false } },
  ],
};

const steps = (parameters: Record<string, number | boolean>) => ({ steps: [{ tool: 'bxt', parameters }] });

describe('rcAstroProblem', () => {
  it('accepts values inside each range and switches', () => {
    expect(rcAstroProblem(steps({ ss: 0.7, sn: 0.9, tiles: 3, lp: true }), [bxt])).toBeNull();
    expect(rcAstroProblem(null, [bxt])).toBeNull();
  });

  it('names the parameter and its range when a value is outside it', () => {
    expect(rcAstroProblem(steps({ ss: 0.9, sn: 0.9 }), [bxt]))
      .toBe('RC-Astro BlurXTerminator: Sharpen Stars must be between 0 and 0.7');
    expect(rcAstroProblem(steps({ tiles: 2.5 }), [bxt]))
      .toBe('RC-Astro BlurXTerminator: Tiles requires a whole number');
  });

  it('refuses a cleared field even before the schema loads', () => {
    expect(rcAstroProblem(steps({ ss: Number.NaN }), undefined))
      .toBe('Enter a finite value for every RC-Astro parameter');
    expect(rcAstroProblem(steps({ ss: 0.9 }), undefined)).toBeNull();
  });
});
