import { describe, expect, it } from 'vitest';
import { astrobinFilterUrl, parseAstrobinFilterId } from '../astrobin';

describe('parseAstrobinFilterId', () => {
  it('reads a bare id or an equipment page address', () => {
    expect(parseAstrobinFilterId('4049')).toBe(4049);
    expect(parseAstrobinFilterId(' 4049 ')).toBe(4049);
    expect(
      parseAstrobinFilterId('https://app.astrobin.com/equipment/explorer/filter/4049/antlia-v-pro-g-36mm')
    ).toBe(4049);
    expect(parseAstrobinFilterId('https://app.astrobin.com/equipment/explorer/filter/4049')).toBe(4049);
    expect(parseAstrobinFilterId('https://app.astrobin.com/equipment/explorer/filter/4049?x=1')).toBe(
      4049
    );
  });

  it('refuses what is not a filter id', () => {
    expect(parseAstrobinFilterId('')).toBeNull();
    expect(parseAstrobinFilterId('0')).toBeNull();
    expect(parseAstrobinFilterId('Antlia G')).toBeNull();
    expect(parseAstrobinFilterId('https://app.astrobin.com/equipment/explorer/camera/12')).toBeNull();
    expect(parseAstrobinFilterId('https://example.com/filter/12')).toBeNull();
  });

  it('links an id to its explorer page', () => {
    expect(astrobinFilterUrl(4049)).toBe('https://app.astrobin.com/equipment/explorer/filter/4049');
  });
});
