import { afterEach, describe, expect, it, vi } from 'vitest';
import { projectPoint, unprojectPoint, type PathScale, type SkyView } from '../../../utils/skyProjection';
import { countAt, shadeImage, shadeRaster, unpackCounts, type SkyCounts } from '../skyShade';

const AT: PathScale = { cx: 500, cy: 270, scale: 236 };
const VIEWS: SkyView[] = [
  { frame: 'equatorial', mode: 'aitoff', centerLon: 0, centerLat: 0 },
  { frame: 'equatorial', mode: 'aitoff', centerLon: 200, centerLat: 0 },
  // The flat map turned to face a place off the equator.
  { frame: 'equatorial', mode: 'aitoff', centerLon: 75, centerLat: 40 },
  { frame: 'galactic', mode: 'aitoff', centerLon: 0, centerLat: 0 },
  { frame: 'equatorial', mode: 'globe', centerLon: 120, centerLat: 25 },
  { frame: 'galactic', mode: 'globe', centerLon: 300, centerLat: -60 },
];

/** A grid as the server sends it: 2° nodes, 180 by 91, from the south pole. */
function grid(count: (ra: number, dec: number) => number, nights: number): SkyCounts {
  const counts = new Uint16Array(180 * 91);
  for (let row = 0; row < 91; row += 1) for (let col = 0; col < 180; col += 1) counts[row * 180 + col] = count(col * 2, -90 + row * 2);
  return { counts, nights, columns: 180, rows: 91 };
}

describe('reading a place back from the map', () => {
  it('undoes the projection in every frame and shape', () => {
    for (const view of VIEWS) {
      let checked = 0;
      for (let ra = 1; ra < 360; ra += 23) {
        for (let dec = -85; dec <= 85; dec += 17) {
          const drawn = projectPoint(ra, dec, view);
          // The far side of the globe is not on the map.
          if (!drawn.visible) continue;
          const back = unprojectPoint(drawn.x, drawn.y, view);
          expect(back, `${ra},${dec} in ${JSON.stringify(view)}`).not.toBeNull();
          const [backRa, backDec] = back!;
          expect(Math.abs(backDec - dec)).toBeLessThan(1e-6);
          const around = Math.abs(((backRa - ra + 540) % 360) - 180) * Math.cos((dec * Math.PI) / 180);
          expect(around).toBeLessThan(1e-6);
          checked += 1;
        }
      }
      expect(checked).toBeGreaterThan(60);
    }
  });

  it('finds nothing off the sky', () => {
    expect(unprojectPoint(2.01, 0, VIEWS[0])).toBeNull();
    expect(unprojectPoint(1.5, 0.9, VIEWS[0])).toBeNull();
    expect(unprojectPoint(1.01, 0, VIEWS[4])).toBeNull();
    expect(unprojectPoint(0.8, 0.8, VIEWS[4])).toBeNull();
    // The middle of the map is the place it is turned to.
    const [ra, dec] = unprojectPoint(0, 0, VIEWS[2])!;
    expect(ra).toBeCloseTo(75, 6);
    expect(dec).toBeCloseTo(40, 6);
  });
});

describe('the counts', () => {
  it('unpacks two bytes a node and refuses a grid of the wrong size', () => {
    // A year's counts pass 255: 365 is 109 and 1, low byte first.
    const wanted = [0, 10, 255, 256, 364, 365];
    const packed = btoa(wanted.map((count) => String.fromCharCode(count & 255, count >> 8)).join(''));
    expect(unpackCounts(packed, 365, 3, 2)).toEqual({ counts: Uint16Array.from(wanted), nights: 365, columns: 3, rows: 2 });
    expect(unpackCounts(packed, 365, 4, 2)).toBeNull();
    expect(unpackCounts(packed, 365, 6, 2)).toBeNull();
    expect(unpackCounts('not base64!', 365, 3, 2)).toBeNull();
    expect(unpackCounts(packed, 0, 3, 2)).toBeNull();
  });

  it('reads a place between the nodes around it, for the shading', () => {
    const sky = grid((_, dec) => (dec >= 0 ? 31 : 0), 31);
    expect(countAt(sky, 123, 0)).toBe(31);
    expect(countAt(sky, 123, -2)).toBe(0);
    // Half way from a node at 0 to one at 31.
    expect(countAt(sky, 50, -1)).toBeCloseTo(15.5, 6);
    expect(countAt(sky, 50, 90)).toBe(31);
    // Across 0h the last column runs on into the first.
    expect(countAt(sky, 359, 10)).toBe(31);
    expect(countAt(grid((ra) => (ra === 0 ? 10 : 0), 31), 359, 0)).toBeCloseTo(5, 6);
  });
});

describe('the shading picture', () => {
  const alpha = (pixels: Uint8ClampedArray, width: number, x: number, y: number) => pixels[(y * width + x) * 4 + 3];

  it('covers the whole sky with no seam and nothing beyond it, flat and globe', () => {
    const full = grid(() => 31, 31);
    const [width, height] = [200, 108];
    for (const view of VIEWS) {
      const pixels = shadeRaster(full, view, AT, 1000, width, height);
      let on = 0;
      for (let py = 0; py < height; py += 1) {
        for (let px = 0; px < width; px += 1) {
          const x = ((px + 0.5) * 5 - AT.cx) / AT.scale;
          const y = -((py + 0.5) * 5 - AT.cy) / AT.scale;
          const inside = view.mode === 'globe' ? x * x + y * y <= 1 : (x * x) / 4 + y * y <= 1;
          // Every pixel on the sky is full, and every one off it clear.
          expect(alpha(pixels, width, px, py), `${px},${py} in ${JSON.stringify(view)}`).toBe(inside ? 255 : 0);
          if (inside) on += 1;
        }
      }
      expect(on).toBeGreaterThan(view.mode === 'globe' ? 6000 : 12000);
    }
  });

  it('is strongest where the count is highest and clear where it is none', () => {
    // In view every night north of +40°, on half the nights down to 0°.
    const sky = grid((_, dec) => (dec >= 40 ? 30 : dec >= 0 ? 15 : 0), 30);
    const pixels = shadeRaster(sky, VIEWS[0], AT, 1000, 200, 108);
    const at = (ra: number, dec: number) => {
      const drawn = projectPoint(ra, dec, VIEWS[0]);
      return alpha(pixels, 200, Math.floor((AT.cx + drawn.x * AT.scale) / 5), Math.floor((AT.cy - drawn.y * AT.scale) / 5));
    };
    expect(at(10, 70)).toBe(255);
    expect(at(300, 60)).toBe(255);
    expect(at(10, 20)).toBe(128);
    expect(at(170, 20)).toBe(128);
    expect(at(10, -30)).toBe(0);
    expect(at(200, -70)).toBe(0);
    // White throughout: the map tints it.
    expect(pixels[(10 * 200 + 100) * 4]).toBe(255);
  });

  describe('as an image', () => {
    afterEach(() => vi.restoreAllMocks());

    it('has no picture where a canvas cannot be painted on', () => {
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
      expect(shadeImage(grid(() => 1, 1), VIEWS[0], AT, 1000, 20, 10)).toBeNull();
    });

    it('paints the raster on a canvas of its size and hands back its address', () => {
      const put = vi.fn();
      const image = { data: new Uint8ClampedArray(20 * 10 * 4) };
      const context = { createImageData: vi.fn(() => image), putImageData: put } as unknown as CanvasRenderingContext2D;
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(context as never);
      vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockReturnValue('data:image/png;base64,AAAA');
      expect(shadeImage(grid(() => 1, 1), VIEWS[0], AT, 1000, 20, 10)).toBe('data:image/png;base64,AAAA');
      expect(context.createImageData).toHaveBeenCalledWith(20, 10);
      expect(put).toHaveBeenCalledWith(image, 0, 0);
      // The middle of the map is sky, and full: one night of one.
      expect(image.data[(5 * 20 + 10) * 4 + 3]).toBe(255);
      expect(image.data[3]).toBe(0);
    });
  });
});
