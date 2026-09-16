/**
 * One colour per filter, shared by the sky map, its cards, and the
 * timeline, so a red bar means the same thing everywhere on the page.
 */

export type CanonicalFilter = 'L' | 'R' | 'G' | 'B' | 'Ha' | 'OIII' | 'SII' | 'other';

const COLORS: Record<CanonicalFilter, string> = {
  L: '#e6e6e6',
  R: '#ff6b6b',
  G: '#6fe08a',
  B: '#6ea8ff',
  Ha: '#ff4f8b',
  OIII: '#3fd6d6',
  SII: '#ffb347',
  other: '#c8a2ff',
};

/** Fold the many spellings of a filter down to one name. */
export function canonicalFilter(name: string): CanonicalFilter {
  const key = name.trim().toLowerCase().replace(/[\s_-]/g, '');
  if (key === 'l' || key === 'lum' || key === 'luminance' || key === 'clear' || key === 'uvir') return 'L';
  if (key === 'r' || key === 'red') return 'R';
  if (key === 'g' || key === 'green') return 'G';
  if (key === 'b' || key === 'blue') return 'B';
  if (key === 'ha' || key === 'halpha' || key === 'hα' || key === 'h-alpha' || key === 'hydrogen') return 'Ha';
  if (key === 'oiii' || key === 'o3' || key === 'oxygen') return 'OIII';
  if (key === 'sii' || key === 's2' || key === 'sulfur' || key === 'sulphur') return 'SII';
  return 'other';
}

export function filterColor(name: string): string {
  return COLORS[canonicalFilter(name)];
}

function hexToRgb(hex: string): [number, number, number] {
  const value = parseInt(hex.slice(1), 16);
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

/**
 * The colour of a mix of filters, weighted by time. Luminance pulls the mix
 * toward white; a pure narrowband target stays its own colour.
 */
export function blendFilterColors(weights: Array<{ filter: string; weight: number }>): string {
  let total = 0;
  const sum = [0, 0, 0];
  for (const { filter, weight } of weights) {
    if (!(weight > 0)) continue;
    const [r, g, b] = hexToRgb(filterColor(filter));
    sum[0] += r * weight;
    sum[1] += g * weight;
    sum[2] += b * weight;
    total += weight;
  }
  if (total === 0) {
    return COLORS.other;
  }
  return `rgb(${Math.round(sum[0] / total)}, ${Math.round(sum[1] / total)}, ${Math.round(sum[2] / total)})`;
}
