import { describe, expect, it } from 'vitest';
import type { OverlaySolution } from '@seiza/astro-overlay';
import { bitmapPositionUnder, pixelAt, skyAt } from '../imagePointer';

const container = { left: 100, top: 50 } as DOMRect;

describe('image pointer', () => {
  it('places the pointer on the bitmap through the zoom', () => {
    // Zoomed to 200% and panned 20px right: client (140, 70) is bitmap (10, 10).
    expect(bitmapPositionUnder(140, 70, container, { scale: 2, offsetX: 20, offsetY: 0 }))
      .toEqual({ x: 10, y: 10 });
  });

  it('counts the pixel on the image grid behind a reduced preview', () => {
    const bitmap = { width: 600, height: 400 };
    const frame = { width: 6000, height: 4000 };
    expect(pixelAt({ x: 10.25, y: 3.9 }, bitmap, frame)).toEqual({ x: 102, y: 39 });
    expect(pixelAt({ x: 10.25, y: 3.9 }, bitmap)).toEqual({ x: 10, y: 3 });
    expect(pixelAt({ x: -0.1, y: 3 }, bitmap)).toBeNull();
    expect(pixelAt({ x: 600, y: 3 }, bitmap)).toBeNull();
  });

  it('gives the sky position of a pixel centre, across grids of other sizes', () => {
    // North up, east left, 1"/px; the reference pixel (49.5, 24.5) is the
    // centre of a 100x50 grid.
    const solution: OverlaySolution = {
      image_width: 100,
      image_height: 50,
      wcs: {
        crval: [10, 20],
        crpix: [49.5, 24.5],
        cd: [[-1 / 3600, 0], [0, 1 / 3600]],
      },
    };
    const centre = skyAt(solution, { x: 49, y: 24 }, { width: 100, height: 50 });
    expect(centre?.[0]).toBeCloseTo(10 + 0.5 / 3600 / Math.cos((20 * Math.PI) / 180), 8);
    // The centre of pixel (99, 49) on a grid twice the size lies a quarter
    // of a solved pixel past the centre of solved pixel (49, 24), each way.
    const doubled = skyAt(solution, { x: 99, y: 49 }, { width: 200, height: 100 });
    const solved = skyAt(solution, { x: 49, y: 24 }, { width: 100, height: 50 });
    expect(doubled?.[1]).toBeCloseTo(solved![1] + 0.25 / 3600, 8);
    expect(skyAt({ image_width: 100, image_height: 50 }, { x: 1, y: 1 }, { width: 100, height: 50 }))
      .toBeNull();
  });
});
