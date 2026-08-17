import { describe, expect, it } from 'vitest';
import { analyzeCells, tiltSummary } from '../tiltAnalysis';
import type { StarInfo } from '../../api/types';

function star(x: number, y: number, hfr: number, extra: Partial<StarInfo> = {}): StarInfo {
  return {
    x,
    y,
    hfr,
    fwhm: hfr * 2,
    brightness: 1000,
    eccentricity: 0.2,
    ...extra,
  };
}

/** One star in the middle of every 3x3 cell of a 300x300 frame. */
function starPerCell(hfrAt: (row: number, col: number) => number): StarInfo[] {
  const stars: StarInfo[] = [];
  for (let row = 0; row < 3; row++) {
    for (let col = 0; col < 3; col++) {
      stars.push(star(col * 100 + 50, row * 100 + 50, hfrAt(row, col)));
    }
  }
  return stars;
}

describe('analyzeCells', () => {
  it('assigns stars to their region and takes medians', () => {
    const stars = [
      star(10, 10, 2.0),
      star(20, 20, 3.0),
      star(30, 30, 4.0),
      star(290, 290, 5.0),
    ];
    const cells = analyzeCells(stars, 300, 300);
    const topLeft = cells.find((cell) => cell.row === 0 && cell.col === 0)!;
    expect(topLeft.starCount).toBe(3);
    expect(topLeft.medianHfr).toBe(3.0);
    const bottomRight = cells.find((cell) => cell.row === 2 && cell.col === 2)!;
    expect(bottomRight.starCount).toBe(1);
    expect(bottomRight.medianHfr).toBe(5.0);
    const empty = cells.find((cell) => cell.row === 1 && cell.col === 1)!;
    expect(empty.starCount).toBe(0);
    expect(empty.medianHfr).toBeNull();
  });

  it('averages elongation directions over the axial period', () => {
    // Orientations near π and near 0 are almost the same axis; a naive
    // mean would point them perpendicular instead.
    const stars = [
      star(10, 10, 2, { theta: 0.05, eccentricity: 0.5 }),
      star(20, 20, 2, { theta: Math.PI - 0.05, eccentricity: 0.5 }),
    ];
    const cells = analyzeCells(stars, 300, 300);
    const cell = cells.find((c) => c.row === 0 && c.col === 0)!;
    expect(cell.meanTheta).not.toBeNull();
    const axisError = Math.min(cell.meanTheta!, Math.PI - cell.meanTheta!);
    expect(axisError).toBeLessThan(0.01);
    expect(cell.thetaCoherence).toBeGreaterThan(0.9);
  });

  it('reports low coherence for random directions', () => {
    const stars = [0, 1, 2, 3].map((i) =>
      star(10 + i, 10 + i, 2, { theta: (i * Math.PI) / 4, eccentricity: 0.5 })
    );
    const cells = analyzeCells(stars, 300, 300);
    const cell = cells.find((c) => c.row === 0 && c.col === 0)!;
    expect(cell.thetaCoherence).toBeLessThan(0.3);
  });
});

describe('tiltSummary', () => {
  it('derives tilt from corner spread against the mean', () => {
    // One soft corner: classic tilt signature.
    const cells = analyzeCells(
      starPerCell((row, col) => (row === 0 && col === 0 ? 3.0 : 2.0)),
      300,
      300
    );
    const summary = tiltSummary(cells);
    expect(summary.worstCorner).toBe('top-left');
    expect(summary.bestCorner).toBeTruthy();
    expect(summary.tiltPercent).toBeCloseTo(50, 0);
  });

  it('derives curvature from corners against the center', () => {
    // All corners equally soft, sharp center: curvature, near-zero tilt.
    const cells = analyzeCells(
      starPerCell((row, col) =>
        (row === 1 && col === 1) ? 2.0 : (row !== 1 && col !== 1) ? 3.0 : 2.5
      ),
      300,
      300
    );
    const summary = tiltSummary(cells);
    expect(summary.tiltPercent).toBeCloseTo(0, 5);
    expect(summary.curvaturePercent).toBeCloseTo(50, 0);
  });

  it('refuses a tilt number with an empty corner', () => {
    const stars = starPerCell(() => 2.0).filter(
      (candidate) => !(candidate.x < 100 && candidate.y < 100)
    );
    const summary = tiltSummary(analyzeCells(stars, 300, 300));
    expect(summary.tiltPercent).toBeNull();
    expect(summary.worstCorner).toBeNull();
  });
});
