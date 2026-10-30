import { describe, expect, it } from 'vitest';
import {
  PIXEL_TILE,
  decodePixelWindow,
  formatSample,
  sampleAt,
  tileFor,
  type PixelWindow,
} from '../pixelWindow';

/** A window as the server encodes it: little-endian f32, base64. */
function encoded(values: number[], window: Omit<PixelWindow, 'values'>): PixelWindow {
  const bytes = new Uint8Array(values.length * 4);
  const view = new DataView(bytes.buffer);
  values.forEach((value, index) => view.setFloat32(index * 4, value, true));
  return { ...window, values: btoa(String.fromCharCode(...bytes)) };
}

describe('pixel windows', () => {
  it('reads each plane of a pixel from the server encoding', () => {
    // A 2x2 RGB window at (10, 20): plane by plane, then row by row.
    const window = decodePixelWindow(encoded(
      [1, 2, 3, 4, 10, 20, 30, 40, 0.5, Number.NaN, 0.25, 0.125],
      { image_width: 100, image_height: 80, planes: 3, x: 10, y: 20, width: 2, height: 2 },
    ));
    expect(sampleAt(window, 11, 20)).toEqual([2, 20, Number.NaN]);
    expect(sampleAt(window, 10, 21)).toEqual([3, 30, 0.25]);
    expect(sampleAt(window, 12, 20)).toBeNull();
    expect(sampleAt(window, 10, 19)).toBeNull();
  });

  it('asks for the aligned tile that holds a pixel', () => {
    expect(tileFor(0, 0)).toEqual({ x: 0, y: 0, width: PIXEL_TILE, height: PIXEL_TILE });
    expect(tileFor(PIXEL_TILE, 2 * PIXEL_TILE - 1)).toMatchObject({ x: PIXEL_TILE, y: PIXEL_TILE });
  });

  it('formats normalized and ADU samples readably', () => {
    expect(formatSample(0.0123456)).toBe('0.012346');
    expect(formatSample(1234.56)).toBe('1234.6');
    expect(formatSample(65535)).toBe('65535');
    expect(formatSample(12345.67)).toBe('12345.7');
    expect(formatSample(123456)).toBe('123456');
    expect(formatSample(0.0000123)).toBe('1.230e-5');
    expect(formatSample(0)).toBe('0');
    expect(formatSample(Number.NaN)).toBe('—');
  });
});
