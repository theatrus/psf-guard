// Computes tiles for the web baseline. This is a port of render/src/synth.rs
// with a 32-bit hash, so the pixels differ from the native spike but the
// work per tile is of the same kind.
const DOC_W = 30000, DOC_H = 20000, BANDS = 8, TILE = 512, STAR_CELL = 256;
const BAND_MIX = [[.5,.3,.2],[.9,.1,0],[.1,.8,.1],[.1,.1,.8],[1,0,.1],[0,1,.2],[.3,0,1],[.6,.4,0]];

function hash(v) {
  v = Math.imul(v ^ (v >>> 16), 0x7feb352d);
  v = Math.imul(v ^ (v >>> 15), 0x846ca68b);
  return (v ^ (v >>> 16)) >>> 0;
}
const unit = (v) => hash(v) / 4294967296;
const lattice = (ix, iy, seed) => unit(Math.imul(ix, 0x9e3779b1) ^ Math.imul(iy, 0x85ebca6b) ^ seed);
function valueNoise(x, y, seed) {
  const fx = Math.floor(x), fy = Math.floor(y), tx = x - fx, ty = y - fy;
  const sx = tx * tx * (3 - 2 * tx), sy = ty * ty * (3 - 2 * ty);
  const top = lattice(fx, fy, seed) * (1 - sx) + lattice(fx + 1, fy, seed) * sx;
  const bottom = lattice(fx, fy + 1, seed) * (1 - sx) + lattice(fx + 1, fy + 1, seed) * sx;
  return top * (1 - sy) + bottom * sy;
}
function fbm(x, y, seed) {
  let sum = 0, amplitude = 0.5, frequency = 1;
  for (let octave = 0; octave < 4; octave++) {
    sum += amplitude * valueNoise(x * frequency, y * frequency, seed + octave);
    amplitude *= 0.5; frequency *= 2.1;
  }
  return sum;
}

// Float32 to half-float bits, through a shared buffer.
const f32 = new Float32Array(1), u32 = new Uint32Array(f32.buffer);
function toHalf(value) {
  f32[0] = value;
  const bits = u32[0], sign = (bits >>> 16) & 0x8000;
  let exponent = ((bits >>> 23) & 0xff) - 112, mantissa = bits & 0x7fffff;
  if (exponent <= 0) return exponent < -10 ? sign : sign | ((mantissa | 0x800000) >>> (14 - exponent));
  if (exponent >= 31) return sign | 0x7c00;
  return sign | (exponent << 10) | (mantissa >>> 13);
}

function generateTile(level, tx, ty) {
  const scale = 1 << level, originX = tx * TILE * scale, originY = ty * TILE * scale;
  const plane = TILE * TILE, bands = new Float32Array(plane * BANDS);
  const field = (value) => Math.max(value - 0.42, 0) ** 2 * 0.6;
  for (let row = 0; row < TILE; row++) {
    const docY = originY + (row + 0.5) * scale;
    if (docY >= DOC_H) break;
    for (let column = 0; column < TILE; column++) {
      const docX = originX + (column + 0.5) * scale;
      if (docX >= DOC_W) break;
      const u = docX / 5000, v = docY / 5000;
      const a = field(fbm(u, v, 11)), b = field(fbm(u + 7.3, v - 2.1, 23)), c = field(fbm(u - 4.4, v + 9.7, 37));
      const pixel = Math.imul(docY | 0, 30011) ^ (docX | 0);
      for (let band = 0; band < BANDS; band++) {
        const mix = BAND_MIX[band];
        const noise = (unit(pixel ^ (band << 28)) - 0.5) * 0.008 / scale;
        bands[band * plane + row * TILE + column] = 0.02 + a * mix[0] + b * mix[1] + c * mix[2] + noise;
      }
    }
  }
  const span = TILE * scale, margin = 16 * Math.max(scale, 1);
  const cx0 = Math.max(Math.floor((originX - margin) / STAR_CELL), 0), cy0 = Math.max(Math.floor((originY - margin) / STAR_CELL), 0);
  const cx1 = Math.min(Math.floor((originX + span + margin) / STAR_CELL), Math.floor(DOC_W / STAR_CELL));
  const cy1 = Math.min(Math.floor((originY + span + margin) / STAR_CELL), Math.floor(DOC_H / STAR_CELL));
  for (let cy = cy0; cy <= cy1; cy++) for (let cx = cx0; cx <= cx1; cx++) {
    const seed = (Math.imul(cy, 65537) ^ cx ^ 0x57415253) >>> 0, count = hash(seed) % 3;
    for (let index = 0; index < count; index++) {
      const star = (Math.imul(seed, 31) + index * 977) >>> 0;
      const docX = (cx + unit(star)) * STAR_CELL, docY = (cy + unit(star + 1)) * STAR_CELL;
      const flux = unit(star + 2) ** 6 * 40 + 0.4, sigma = 1.4 + unit(star + 3) * 1.2;
      const levelSigma = Math.max(sigma / scale, 0.6);
      const amplitude = flux / (2 * Math.PI * sigma * sigma) * (sigma / scale / levelSigma) ** 2;
      if (amplitude < 0.002) continue;
      const centerX = (docX - originX) / scale, centerY = (docY - originY) / scale, reach = Math.ceil(levelSigma * 4);
      const x0 = Math.max(Math.floor(centerX - reach), 0), y0 = Math.max(Math.floor(centerY - reach), 0);
      const x1 = Math.min(Math.max(Math.ceil(centerX + reach), 0), TILE), y1 = Math.min(Math.max(Math.ceil(centerY + reach), 0), TILE);
      const tint = unit(star + 4);
      for (let row = y0; row < y1; row++) for (let column = x0; column < x1; column++) {
        const dx = column + 0.5 - centerX, dy = row + 0.5 - centerY;
        const value = amplitude * Math.exp(-(dx * dx + dy * dy) / (2 * levelSigma * levelSigma));
        for (let band = 0; band < BANDS; band++) {
          bands[band * plane + row * TILE + column] += value * (0.6 + 0.4 * Math.abs(Math.sin(tint * (band + 1))));
        }
      }
    }
  }
  const half = new Uint16Array(bands.length);
  for (let index = 0; index < bands.length; index++) half[index] = toHalf(bands[index]);
  return half;
}

onmessage = ({ data: { level, x, y } }) => {
  const started = performance.now();
  const samples = generateTile(level, x, y);
  postMessage({ level, x, y, samples, ms: performance.now() - started }, [samples.buffer]);
};
