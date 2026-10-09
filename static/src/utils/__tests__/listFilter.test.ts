import { describe, expect, it } from 'vitest';
import { listFilterLabel, listFilterOf } from '../listFilter';

describe('list filter', () => {
  it('reads one value or several, and all or nothing as every value', () => {
    expect(listFilterOf(null)).toEqual([]);
    expect(listFilterOf('')).toEqual([]);
    expect(listFilterOf('all')).toEqual([]);
    expect(listFilterOf('Ha')).toEqual(['Ha']);
    expect(listFilterOf('Ha,OIII,Ha')).toEqual(['Ha', 'OIII']);
    // A value with a comma of its own travels encoded.
    expect(listFilterOf(`${encodeURIComponent('Ha,7nm')},L`)).toEqual(['Ha,7nm', 'L']);
    expect(listFilterOf('50%')).toEqual(['50%']);
    expect(listFilterOf('Rotation_Skew', (value) => value.toLowerCase())).toEqual(['rotation_skew']);
  });

  it('says what it keeps in a few words', () => {
    expect(listFilterLabel([])).toBe('All');
    expect(listFilterLabel(['Ha', 'OIII'])).toBe('Ha, OIII');
    expect(listFilterLabel(['L', 'R', 'G'])).toBe('3 chosen');
    expect(listFilterLabel(['rotation_skew'], (value) => value.toUpperCase())).toBe('ROTATION_SKEW');
  });
});
