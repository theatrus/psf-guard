/**
 * Linear samples the server reads from a small rectangle of an image file,
 * for the viewers' pointer readout. The server reads only that rectangle's
 * rows, so a readout over a large stack never downloads the stack.
 */

/** A rectangle of samples as the server sends it. */
export interface PixelWindow {
  /** The whole image's size and plane count. */
  image_width: number;
  image_height: number;
  planes: number;
  /** The part of the request that lies on the image. */
  x: number;
  y: number;
  width: number;
  height: number;
  /** Little-endian f32 samples, base64: plane by plane, then row by row. */
  values: string;
}

export interface DecodedWindow extends Omit<PixelWindow, 'values'> {
  values: Float32Array;
}

export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Edge of the square tiles the readout asks for. */
export const PIXEL_TILE = 64;

export function decodePixelWindow(window: PixelWindow): DecodedWindow {
  const binary = atob(window.values);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  const view = new DataView(bytes.buffer);
  const values = new Float32Array(Math.floor(bytes.length / 4));
  for (let index = 0; index < values.length; index += 1) {
    values[index] = view.getFloat32(index * 4, true);
  }
  return { ...window, values };
}

/** Each plane's sample at a pixel of the window, or null off the window. */
export function sampleAt(window: DecodedWindow, x: number, y: number): number[] | null {
  const column = x - window.x;
  const row = y - window.y;
  if (column < 0 || row < 0 || column >= window.width || row >= window.height) return null;
  const plane = window.width * window.height;
  return Array.from({ length: window.planes }, (_, index) =>
    window.values[index * plane + row * window.width + column]);
}

/** The tile that holds a pixel. */
export function tileFor(x: number, y: number): PixelRect {
  return {
    x: Math.floor(x / PIXEL_TILE) * PIXEL_TILE,
    y: Math.floor(y / PIXEL_TILE) * PIXEL_TILE,
    width: PIXEL_TILE,
    height: PIXEL_TILE,
  };
}

/**
 * A sample for the readout: a camera's whole ADU as they are, five
 * significant digits for a normalized stack, one decimal in the thousands,
 * so both read naturally.
 */
export function formatSample(value: number): string {
  if (!Number.isFinite(value)) return '—';
  const magnitude = Math.abs(value);
  if (Number.isInteger(value)) return String(value);
  if (magnitude >= 1000) return value.toFixed(magnitude >= 100000 ? 0 : 1);
  if (magnitude < 1e-4) return value.toExponential(3);
  return value.toPrecision(5);
}
