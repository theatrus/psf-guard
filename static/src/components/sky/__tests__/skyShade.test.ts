import { afterEach, describe, expect, it, vi } from 'vitest';
import { countAt, countsAgainst, coverImage, coverRaster, unpackCounts, type SkyCounts } from '../skyShade';

/** A grid as the server sends it: 2° nodes, 180 by 91, from the south pole. */
function grid(count: (ra: number, dec: number) => number, nights: number): SkyCounts {
  const counts = new Uint16Array(180 * 91);
  for (let row = 0; row < 91; row += 1) for (let col = 0; col < 180; col += 1) counts[row * 180 + col] = count(col * 2, -90 + row * 2);
  return { counts, nights, columns: 180, rows: 91 };
}

/** A plain drawing of the sky, 360 units wide and 180 high: right
 *  ascension across, north at the top, and no sky in its last 20 units. */
const plain = (turned = 0) => (x: number, y: number): readonly [number, number] | null => (x >= 340 ? null : [x + turned, 90 - y]);

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

  it('reads a place between its four nodes, across 0h and up to the poles', () => {
    const sky = grid((ra, dec) => (dec >= 0 ? 10 : 0) + (ra === 0 ? 4 : 0), 14);
    expect(countAt(sky, 100, 40)).toBe(10);
    expect(countAt(sky, 100, -1)).toBe(5);
    // Half way from the last column back to the first.
    expect(countAt(sky, 359, 40)).toBe(12);
    expect(countAt(sky, -1, 40)).toBe(12);
    expect(countAt(sky, 100, 90)).toBe(10);
    expect(countAt(sky, 100, -90)).toBe(0);
  });
});

describe('the cover', () => {
  const alpha = (pixels: Uint8ClampedArray, width: number, x: number, y: number) => pixels[(y * width + x) * 4 + 3];

  it('covers sky more the fewer nights it is in view, leaves sky in view every night clear, and nothing where there is no sky', () => {
    // In view every night north of +40°, on half the nights down to 0°.
    const sky = grid((_, dec) => (dec >= 40 ? 30 : dec >= 0 ? 15 : 0), 30);
    // Four units a pixel: too coarse for the hatch.
    const pixels = coverRaster(sky, 360, 180, 90, 45, plain());
    const at = (ra: number, dec: number) => alpha(pixels, 90, Math.floor(ra / 4), Math.floor((90 - dec) / 4));
    expect(at(10, 70)).toBe(0);
    expect(at(300, 60)).toBe(0);
    expect(at(10, 20)).toBe(128);
    expect(at(170, 20)).toBe(128);
    // Sky never in view is covered whole.
    expect(at(10, -30)).toBe(255);
    expect(at(200, -70)).toBe(255);
    // Past the edge of the sky the drawing has, nothing is covered.
    expect(at(350, -30)).toBe(0);
    expect(at(350, 20)).toBe(0);
    // White throughout: the stylesheet gives the cover its colour.
    expect(pixels[(30 * 90 + 20) * 4]).toBe(255);
  });

  it('hatches sky never in view, in stripes that stay put when the sky moves', () => {
    // Never in view south of the equator; half the nights north of it.
    const sky = grid((_, dec) => (dec >= 0 ? 15 : 0), 30);
    // 2.5 units a pixel.
    const [width, height] = [144, 72];
    const south = (turned: number) => {
      const pixels = coverRaster(sky, 360, 180, width, height, plain(turned));
      return Array.from({ length: 60 }, (_, i) => alpha(pixels, width, 20 + i, 60));
    };
    const row = south(0);
    // The cover goes from whole to about 70% and back, every 12 units:
    // 4.8 pixels of this raster.
    expect(Math.max(...row)).toBeGreaterThanOrEqual(253);
    expect(Math.min(...row)).toBeLessThanOrEqual(182);
    expect(Math.min(...row)).toBeGreaterThanOrEqual(178);
    const tops = row.filter((value, i) => i > 0 && i < 59 && value >= row[i - 1] && value > row[i + 1]).length;
    expect(tops).toBeGreaterThanOrEqual(11);
    expect(tops).toBeLessThanOrEqual(13);
    // The same stripes with the sky turned a quarter of the way round.
    expect(south(90)).toEqual(row);
    // Sky in view on some nights has no hatch.
    const pixels = coverRaster(sky, 360, 180, width, height, plain());
    const north = Array.from({ length: 60 }, (_, i) => alpha(pixels, width, 20 + i, 16));
    expect(new Set(north)).toEqual(new Set([128]));
  });

  it('samples a raster whose pixels are not square by its own scale each way', () => {
    // In view north of the equator, never south of it: the cover's edge
    // is at 90 units down a drawing 180 high, whatever the raster's shape.
    const sky = grid((_, dec) => (dec >= 1 ? 30 : dec <= -1 ? 0 : 15), 30);
    for (const [width, height] of [[90, 45], [90, 50], [90, 37]]) {
      const pixels = coverRaster(sky, 360, 180, width, height, plain());
      const down = Array.from({ length: height }, (_, py) => alpha(pixels, width, 10, py));
      // The first row more than half covered, as a place in the drawing.
      const row = down.findIndex((value) => value > 128);
      expect(Math.abs((row + 0.5) * (180 / height) - 90), `${width} by ${height}`).toBeLessThanOrEqual(180 / height + 1e-9);
    }
  });

  it('weighs a year against the nights that are enough: clear from there up, heavier the further short', () => {
    // Nights in view by declination: 300, 7, 6, 3, 1 and none.
    const year = grid((_, dec) => (dec >= 60 ? 300 : dec >= 40 ? 7 : dec >= 20 ? 6 : dec >= 0 ? 3 : dec >= -20 ? 1 : 0), 365);
    const week = countsAgainst(year, 7);
    expect(week.nights).toBe(7);
    expect(Math.max(...week.counts)).toBe(7);
    // The year's own counts are left as they were.
    expect(Math.max(...year.counts)).toBe(300);
    const pixels = coverRaster(week, 360, 180, 90, 45, plain());
    const at = (dec: number) => alpha(pixels, 90, 10, Math.floor((90 - dec) / 4));
    // Seven nights or more is clear, whether 7 or 300.
    expect(at(70)).toBe(0);
    expect(at(50)).toBe(0);
    // Six, three and one night of the seven: a seventh, four sevenths and
    // six sevenths covered.
    expect(at(30)).toBe(Math.round(255 / 7));
    expect(at(10)).toBe(Math.round((255 * 4) / 7));
    expect(at(-10)).toBe(Math.round((255 * 6) / 7));
    // None: covered whole.
    expect(at(-50)).toBe(255);
    // Against all 365 nights the same sky would be nearly all covered.
    const whole = coverRaster(year, 360, 180, 90, 45, plain());
    expect(alpha(whole, 90, 10, Math.floor((90 - 50) / 4))).toBe(250);
  });

  describe('as an image', () => {
    afterEach(() => vi.restoreAllMocks());

    it('is nothing where there is no canvas to paint on', () => {
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
      expect(coverImage(grid(() => 1, 1), 360, 180, 20, 10, plain())).toBeNull();
    });

    it('paints the raster on a canvas of its size and hands back its address', () => {
      const put = vi.fn();
      const image = { data: new Uint8ClampedArray(20 * 10 * 4) };
      const context = { createImageData: vi.fn(() => image), putImageData: put } as unknown as CanvasRenderingContext2D;
      vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(context as never);
      vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockReturnValue('data:image/png;base64,AAAA');
      expect(coverImage(grid(() => 0, 1), 360, 180, 20, 10, plain())).toBe('data:image/png;base64,AAAA');
      expect(context.createImageData).toHaveBeenCalledWith(20, 10);
      expect(put).toHaveBeenCalledWith(image, 0, 0);
      // The middle of the drawing is sky, and covered whole: never in view.
      expect(image.data[(5 * 20 + 10) * 4 + 3]).toBe(255);
      // Its right edge has no sky.
      expect(image.data[(5 * 20 + 19) * 4 + 3]).toBe(0);
    });
  });
});
