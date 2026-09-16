import { describe, expect, it } from 'vitest';
import {
  aitoff,
  equatorialToGalactic,
  footprintOutline,
  formatDecShort,
  formatRaShort,
  galacticToEquatorial,
  moonIllumination,
  projectedPath,
  wrap180,
  wrap360,
} from '../skyProjection';

describe('aitoff', () => {
  it('puts the centre at the origin and east on the left', () => {
    expect(aitoff(180, 0, 180)).toEqual({ x: -0, y: 0 });
    // Ten degrees east of centre (a larger longitude) lands at negative x.
    expect(aitoff(190, 0, 180).x).toBeLessThan(0);
    expect(aitoff(170, 0, 180).x).toBeGreaterThan(0);
  });

  it('reaches the ellipse edge at the seam and the poles', () => {
    expect(Math.abs(aitoff(0, 0, 180).x)).toBeCloseTo(2, 5);
    expect(aitoff(180, 90, 180).y).toBeCloseTo(1, 5);
    expect(aitoff(180, -90, 180).y).toBeCloseTo(-1, 5);
  });
});

describe('galactic conversion', () => {
  it('sends the galactic centre to Sagittarius and back', () => {
    const [ra, dec] = galacticToEquatorial(0, 0);
    expect(ra).toBeCloseTo(266.405, 1);
    expect(dec).toBeCloseTo(-28.936, 1);
    const [l, b] = equatorialToGalactic(ra, dec);
    expect(wrap180(l)).toBeCloseTo(0, 3);
    expect(b).toBeCloseTo(0, 3);
  });

  it('puts the north celestial pole at the known galactic longitude', () => {
    const [l, b] = equatorialToGalactic(0, 90);
    expect(l).toBeCloseTo(122.932, 2);
    expect(b).toBeCloseTo(27.128, 2);
  });
});

describe('wrapping', () => {
  it('folds longitudes into their ranges', () => {
    expect(wrap360(-30)).toBe(330);
    expect(wrap360(725)).toBe(5);
    expect(wrap180(190)).toBe(-170);
    expect(wrap180(-190)).toBe(170);
  });
});

describe('footprintOutline', () => {
  it('draws a header footprint as a rectangle about the target, wider in RA at high declination', () => {
    const outline = footprintOutline(30, 60, { width_deg: 2, height_deg: 1, rotation_deg: 0 }, 1);
    expect(outline).toHaveLength(4);
    const ras = outline.map(([ra]) => wrap180(ra - 30));
    const decs = outline.map(([, dec]) => dec);
    // Two degrees across the sky at dec 60 spans four degrees of RA.
    expect(Math.max(...ras) - Math.min(...ras)).toBeCloseTo(4, 3);
    expect(Math.max(...decs) - Math.min(...decs)).toBeCloseTo(1, 6);
  });

  it('uses solved vertices as they are', () => {
    const vertices: [number, number][] = [
      [10, 10],
      [11, 10],
      [11, 11],
      [10, 11],
    ];
    const outline = footprintOutline(10.5, 10.5, { width_deg: 1, height_deg: 1, vertices }, 1);
    expect(outline).toEqual(vertices);
  });
});

describe('projectedPath', () => {
  const at = { cx: 500, cy: 250, scale: 200 };

  it('breaks a line that crosses the seam into two strokes', () => {
    const line: [number, number][] = [];
    for (let ra = 340; ra <= 380; ra += 5) line.push([ra % 360, 20]);
    const path = projectedPath(line, 'equatorial', at);
    expect(path.split('M')).toHaveLength(3);
  });

  it('closes a small field into one polygon', () => {
    const path = projectedPath(
      footprintOutline(100, -20, { width_deg: 3, height_deg: 2, rotation_deg: 30 }),
      'equatorial',
      at,
      true
    );
    expect(path.startsWith('M')).toBe(true);
    expect(path.endsWith('Z')).toBe(true);
    expect(path.split('M')).toHaveLength(2);
  });
});

describe('formatting', () => {
  it('prints short sexagesimal coordinates', () => {
    expect(formatRaShort(10.68)).toBe('0h 43m');
    expect(formatRaShort(359.999)).toBe('0h 00m');
    expect(formatDecShort(41.27)).toBe('+41° 16′');
    expect(formatDecShort(-0.5)).toBe('−0° 30′');
  });
});

describe('moonIllumination', () => {
  it('is dark at a known new moon and bright a fortnight later', () => {
    const newMoon = 947182440; // 2000-01-06 18:14 UTC
    expect(moonIllumination(newMoon)).toBeCloseTo(0, 3);
    expect(moonIllumination(newMoon + 14.765 * 86400)).toBeCloseTo(1, 2);
  });
});
