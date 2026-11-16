import { unprojectPoint, type PathScale, type SkyView } from '../../utils/skyProjection';

/**
 * On how many nights each part of the sky is in view: one count for each
 * node of a grid of the whole sky, row by row from the south pole, each
 * row running from 0h of right ascension.
 */
export interface SkyCounts {
  counts: Uint8Array;
  /** The nights counted: the most a node can have. */
  nights: number;
  columns: number;
  rows: number;
}

/** Unpack counts sent as base64, one byte a node. */
export function unpackCounts(base64: string, nights: number, columns: number, rows: number): SkyCounts | null {
  try {
    const text = atob(base64);
    if (text.length !== columns * rows || nights < 1) return null;
    const counts = new Uint8Array(text.length);
    for (let i = 0; i < text.length; i += 1) counts[i] = text.charCodeAt(i);
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
  const across = ((((raDeg % 360) + 360) % 360) / 360) * sky.columns;
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

/** The count at the node nearest a place: a whole number of nights. */
export function nightsAt(sky: SkyCounts, raDeg: number, decDeg: number): number {
  const col = Math.round(((((raDeg % 360) + 360) % 360) / 360) * sky.columns) % sky.columns;
  const row = Math.min(sky.rows - 1, Math.max(0, Math.round(((decDeg + 90) / 180) * (sky.rows - 1))));
  return sky.counts[row * sky.columns + col];
}

/**
 * The shading as a picture of the map: for each pixel of a `width` by
 * `height` raster laid over a map `mapWidth` wide, how much of the nights
 * the sky under it is in view, as the alpha of a white pixel (RGBA). Each
 * pixel is looked up through the projection the other way round, so the
 * picture has no seam to break at and no limb to spill over: a pixel off
 * the sky is clear.
 */
export function shadeRaster(sky: SkyCounts, view: SkyView, at: PathScale, mapWidth: number, width: number, height: number): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(width * height * 4);
  const perPixel = mapWidth / width;
  for (let py = 0; py < height; py += 1) {
    const y = -((py + 0.5) * perPixel - at.cy) / at.scale;
    for (let px = 0; px < width; px += 1) {
      const x = ((px + 0.5) * perPixel - at.cx) / at.scale;
      const place = unprojectPoint(x, y, view);
      if (!place) continue;
      const share = countAt(sky, place[0], place[1]) / sky.nights;
      if (share <= 0) continue;
      const i = (py * width + px) * 4;
      pixels[i] = 255;
      pixels[i + 1] = 255;
      pixels[i + 2] = 255;
      pixels[i + 3] = Math.round(255 * Math.min(1, share));
    }
  }
  return pixels;
}

/**
 * [`shadeRaster`] as an image address for the map to draw, or null where
 * there is no canvas to paint on, as under the unit tests.
 */
export function shadeImage(sky: SkyCounts, view: SkyView, at: PathScale, mapWidth: number, width: number, height: number): string | null {
  if (typeof document === 'undefined' || (typeof navigator !== 'undefined' && /jsdom/i.test(navigator.userAgent))) return null;
  try {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext('2d');
    if (!context) return null;
    const image = context.createImageData(width, height);
    image.data.set(shadeRaster(sky, view, at, mapWidth, width, height));
    context.putImageData(image, 0, 0);
    return canvas.toDataURL('image/png');
  } catch {
    return null;
  }
}
