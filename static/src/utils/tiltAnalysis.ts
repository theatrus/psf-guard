import type { StarInfo } from '../api/types';

/**
 * Sensor-region star statistics for tilt and aberration inspection.
 *
 * The frame divides into a 3x3 grid (the layout ASTAP's HFD inspection and
 * PixInsight's aberration mosaic both use). Each cell aggregates the stars
 * detected inside it; the summary compares corner cells against the center
 * the way ASTAP derives its tilt numbers.
 */

export interface CellStats {
  row: number;
  col: number;
  starCount: number;
  medianHfr: number | null;
  medianEccentricity: number | null;
  /** Mean elongation direction in radians over [0, π). Orientation is
   * axial (a star elongated at θ looks the same at θ+π), so this is the
   * circular mean over doubled angles. Null without fitted PSFs. */
  meanTheta: number | null;
  /** Agreement of elongation directions, 0 (random) to 1 (aligned).
   * Aligned directions across a region point at astigmatism or tilt;
   * random directions are seeing noise. */
  thetaCoherence: number;
}

export interface CornerHfr {
  corner: 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
  hfr: number | null;
}

export interface TiltSummary {
  centerHfr: number | null;
  corners: CornerHfr[];
  /** Median HFR over every cell that has stars. */
  meanHfr: number | null;
  /** (worst corner − best corner) / mean HFR, as a percentage. ASTAP's
   * tilt indicator: one soft corner against a sharp opposite one. */
  tiltPercent: number | null;
  /** mean(corners) / center − 1 as a percentage. Uniformly soft corners
   * with a sharp center indicate field curvature, not tilt. */
  curvaturePercent: number | null;
  worstCorner: CornerHfr['corner'] | null;
  bestCorner: CornerHfr['corner'] | null;
}

function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[mid]
    : (sorted[mid - 1] + sorted[mid]) / 2;
}

export function analyzeCells(
  stars: StarInfo[],
  width: number,
  height: number
): CellStats[] {
  const cells: CellStats[] = [];
  for (let row = 0; row < 3; row++) {
    for (let col = 0; col < 3; col++) {
      const x0 = (col * width) / 3;
      const x1 = ((col + 1) * width) / 3;
      const y0 = (row * height) / 3;
      const y1 = ((row + 1) * height) / 3;
      const cellStars = stars.filter(
        (star) => star.x >= x0 && star.x < x1 && star.y >= y0 && star.y < y1
      );
      const withTheta = cellStars.filter(
        (star): star is StarInfo & { theta: number } =>
          typeof star.theta === 'number' && star.eccentricity > 0
      );
      // Circular mean over doubled angles: orientation has period π.
      let meanTheta: number | null = null;
      let thetaCoherence = 0;
      if (withTheta.length > 0) {
        let sumCos = 0;
        let sumSin = 0;
        for (const star of withTheta) {
          // Weight by eccentricity: a round star has no direction to vote.
          sumCos += Math.cos(2 * star.theta) * star.eccentricity;
          sumSin += Math.sin(2 * star.theta) * star.eccentricity;
        }
        const weight = withTheta.reduce((sum, star) => sum + star.eccentricity, 0);
        const magnitude = Math.hypot(sumCos, sumSin);
        if (weight > 0 && magnitude > 1e-9) {
          thetaCoherence = magnitude / weight;
          let angle = Math.atan2(sumSin, sumCos) / 2;
          if (angle < 0) angle += Math.PI;
          meanTheta = angle;
        }
      }
      cells.push({
        row,
        col,
        starCount: cellStars.length,
        medianHfr: median(cellStars.map((star) => star.hfr)),
        medianEccentricity: median(cellStars.map((star) => star.eccentricity)),
        meanTheta,
        thetaCoherence,
      });
    }
  }
  return cells;
}

const CORNER_CELLS: Array<{ corner: CornerHfr['corner']; row: number; col: number }> = [
  { corner: 'top-left', row: 0, col: 0 },
  { corner: 'top-right', row: 0, col: 2 },
  { corner: 'bottom-left', row: 2, col: 0 },
  { corner: 'bottom-right', row: 2, col: 2 },
];

export function tiltSummary(cells: CellStats[]): TiltSummary {
  const cellAt = (row: number, col: number) =>
    cells.find((cell) => cell.row === row && cell.col === col);
  const centerHfr = cellAt(1, 1)?.medianHfr ?? null;
  const corners: CornerHfr[] = CORNER_CELLS.map(({ corner, row, col }) => ({
    corner,
    hfr: cellAt(row, col)?.medianHfr ?? null,
  }));
  const meanHfr = median(
    cells.map((cell) => cell.medianHfr).filter((hfr): hfr is number => hfr !== null)
  );

  const measured = corners.filter(
    (corner): corner is { corner: CornerHfr['corner']; hfr: number } =>
      corner.hfr !== null
  );
  let tiltPercent: number | null = null;
  let worstCorner: CornerHfr['corner'] | null = null;
  let bestCorner: CornerHfr['corner'] | null = null;
  // Tilt is a differential measure; with corners missing it would compare
  // a corner against nothing and report noise.
  if (measured.length === 4 && meanHfr !== null && meanHfr > 0) {
    const sorted = [...measured].sort((a, b) => a.hfr - b.hfr);
    bestCorner = sorted[0].corner;
    worstCorner = sorted[sorted.length - 1].corner;
    tiltPercent =
      ((sorted[sorted.length - 1].hfr - sorted[0].hfr) / meanHfr) * 100;
  }

  let curvaturePercent: number | null = null;
  if (measured.length === 4 && centerHfr !== null && centerHfr > 0) {
    const cornerMean =
      measured.reduce((sum, corner) => sum + corner.hfr, 0) / measured.length;
    curvaturePercent = (cornerMean / centerHfr - 1) * 100;
  }

  return {
    centerHfr,
    corners,
    meanHfr,
    tiltPercent,
    curvaturePercent,
    worstCorner,
    bestCorner,
  };
}
