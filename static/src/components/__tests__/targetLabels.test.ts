import { describe, expect, it } from 'vitest';
import { fittedWidth, sharedStart, shortLabel } from '../header/targetLabels';

describe('target labels', () => {
  it("drops the long start a mosaic's panels share, keeping a word before the number", () => {
    const panels = ['Heart and Soul Nebula Panel 1', 'Heart and Soul Nebula Panel 2', 'Heart and Soul Nebula Panel 3'];
    const prefix = sharedStart(panels);
    expect(prefix).toBe('Heart and Soul Nebula');
    expect(panels.map(name => shortLabel(name, prefix))).toEqual(['Panel 1', 'Panel 2', 'Panel 3']);
    // A short shared start is not worth dropping.
    expect(sharedStart(['M31 panel 1', 'M31 panel 2'])).toBe('');
  });

  it('leaves names whole when they share little or nothing', () => {
    expect(sharedStart(['Veil east', 'Veil west'])).toBe('');
    expect(sharedStart(['NGC 7000', 'IC 5070'])).toBe('');
    expect(sharedStart(['Only one target'])).toBe('');
    expect(shortLabel('Veil east', '')).toBe('Veil east');
  });

  it('sizes a closed select to its text, within reason', () => {
    expect(fittedWidth('Panel 1')).toBe('clamp(7rem, calc(7ch + 2.75rem), 24rem)');
  });
});
