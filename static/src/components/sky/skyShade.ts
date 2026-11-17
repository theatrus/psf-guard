import { wrap360 } from '../../utils/skyProjection';

/**
 * On how many nights each part of the sky is in view: one count for each
 * node of a grid of the whole sky, row by row from the south pole, each
 * row running from 0h of right ascension.
 */
export interface SkyCounts {
  counts: Uint16Array;
  /** The nights counted: the most a node can have. */
  nights: number;
  columns: number;
  rows: number;
}

/** Unpack counts sent as base64, two bytes a node with the low byte first. */
export function unpackCounts(base64: string, nights: number, columns: number, rows: number): SkyCounts | null {
  try {
    const text = atob(base64);
    if (text.length !== columns * rows * 2 || nights < 1) return null;
    const counts = new Uint16Array(columns * rows);
    for (let i = 0; i < counts.length; i += 1) counts[i] = text.charCodeAt(2 * i) | (text.charCodeAt(2 * i + 1) << 8);
    return { counts, nights, columns, rows };
  } catch {
    return null;
  }
}

/**
 * The count at a place, drawn between the four nodes around it so the
 * shading has no steps. Right ascension wraps; declination stops at the
 * poles.
 */
export function countAt(sky: SkyCounts, raDeg: number, decDeg: number): number {
  const across = (wrap360(raDeg) / 360) * sky.columns;
  const up = Math.min(sky.rows - 1, Math.max(0, ((decDeg + 90) / 180) * (sky.rows - 1)));
  const col = Math.floor(across);
  const row = Math.min(sky.rows - 2, Math.floor(up));
  const fx = across - col;
  const fy = up - row;
  const at = (c: number, r: number) => sky.counts[r * sky.columns + (c % sky.columns)];
  const low = at(col, row) * (1 - fx) + at(col + 1, row) * fx;
  const high = at(col, row + 1) * (1 - fx) + at(col + 1, row + 1) * fx;
  return low * (1 - fy) + high * fy;
}

/** The hatch over sky never in view: stripes this many drawing units
 *  apart, each letting this much of the cover go at its middle. */
const HATCH_PERIOD = 12;
const HATCH_DEPTH = 0.3;
/** The hatch is left out of a raster too coarse to hold it: one whose
 *  pixels are wider than this many drawing units. */
const HATCH_FINEST_PIXEL = 3;

/**
 * The cover as a picture of a drawing of the sky: for each pixel of a
 * `width` by `height` raster laid over a drawing `drawnWidth` units wide,
 * how much of the sky under it is covered, as the alpha of a white pixel
 * (RGBA). Sky in view every night is clear; sky in view on fewer nights is
 * covered more, in proportion; sky never in view is covered whole and
 * hatched with diagonal stripes, so it reads as out of reach without its
 * colour.
 *
 * `placeAt(x, y)` is the sky drawn at a point of the drawing, in its
 * units, as `[ra, dec]` in degrees, or null where the drawing has no sky:
 * the drawing's projection the other way round. Looking each pixel up
 * through it leaves no seam and no edge to spill over. The hatch runs in
 * the drawing's units, so it stays put while the sky moves under it.
 */
export function coverRaster(sky: SkyCounts, drawnWidth: number, width: number, height: number, placeAt: (x: number, y: number) => readonly [number, number] | null): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(width * height * 4);
  const perPixel = drawnWidth / width;
  const hatched = perPixel <= HATCH_FINEST_PIXEL;
  for (let py = 0; py < height; py += 1) {
    for (let px = 0; px < width; px += 1) {
      const place = placeAt((px + 0.5) * perPixel, (py + 0.5) * perPixel);
      if (!place) continue;
      const share = countAt(sky, place[0], place[1]) / sky.nights;
      if (share >= 1) continue;
      let cover = 1 - Math.max(0, share);
      if (share <= 0 && hatched) {
        const across = ((px + py + 1) * perPixel) / HATCH_PERIOD;
        cover -= HATCH_DEPTH * (0.5 + 0.5 * Math.cos(2 * Math.PI * across));
      }
      const i = (py * width + px) * 4;
      pixels[i] = 255;
      pixels[i + 1] = 255;
      pixels[i + 2] = 255;
      pixels[i + 3] = Math.round(255 * cover);
    }
  }
  return pixels;
}

/**
 * [`coverRaster`] as an image address for a drawing to show, or null where
 * there is no canvas to paint on.
 */
export function coverImage(sky: SkyCounts, drawnWidth: number, width: number, height: number, placeAt: (x: number, y: number) => readonly [number, number] | null): string | null {
  if (typeof document === 'undefined') return null;
  try {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext('2d');
    if (!context) return null;
    const image = context.createImageData(width, height);
    image.data.set(coverRaster(sky, drawnWidth, width, height, placeAt));
    context.putImageData(image, 0, 0);
    return canvas.toDataURL('image/png');
  } catch {
    return null;
  }
}
