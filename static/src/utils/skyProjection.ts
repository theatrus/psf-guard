/**
 * All-sky projection helpers for the Sky coverage page.
 *
 * Longitudes and latitudes are degrees. Projected points come back in Aitoff
 * unit coordinates, x in [-2, 2] and y in [-1, 1], with east to the LEFT the
 * way a star chart is drawn; the map scales them to pixels.
 */

export type SkyFrame = 'equatorial' | 'galactic';

export interface SkyPoint {
  x: number;
  y: number;
}

/** A longitude/latitude pair in the current frame, degrees. */
export type LonLat = [number, number];

const DEG = Math.PI / 180;
const RAD = 180 / Math.PI;

/** Wrap a longitude difference into [-180, 180). */
export function wrap180(deg: number): number {
  return ((((deg + 180) % 360) + 360) % 360) - 180;
}

/** Wrap a longitude into [0, 360). */
export function wrap360(deg: number): number {
  return ((deg % 360) + 360) % 360;
}

/** Aitoff projection about `centerLon`, east to the left. */
export function aitoff(lonDeg: number, latDeg: number, centerLonDeg = 0): SkyPoint {
  const lambda = wrap180(lonDeg - centerLonDeg) * DEG;
  const phi = latDeg * DEG;
  const cosPhi = Math.cos(phi);
  const alpha = Math.acos(Math.min(1, Math.max(-1, cosPhi * Math.cos(lambda / 2))));
  const sinc = alpha === 0 ? 1 : Math.sin(alpha) / alpha;
  // Aitoff spans [-π, π] by [-π/2, π/2]; scale to [-2, 2] by [-1, 1] so the
  // outline is the ellipse x²/4 + y² = 1.
  const unit = 2 / Math.PI;
  return {
    x: (-(2 * cosPhi * Math.sin(lambda / 2)) / sinc) * unit,
    y: (Math.sin(phi) / sinc) * unit,
  };
}

// J2000 galactic pole and the galactic longitude of the celestial pole.
const GALACTIC_POLE_RA = 192.85948;
const GALACTIC_POLE_DEC = 27.12825;
const GALACTIC_LON_OF_NCP = 122.93192;

/** ICRS right ascension and declination to galactic longitude and latitude. */
export function equatorialToGalactic(raDeg: number, decDeg: number): LonLat {
  const ra = raDeg * DEG;
  const dec = decDeg * DEG;
  const poleRa = GALACTIC_POLE_RA * DEG;
  const poleDec = GALACTIC_POLE_DEC * DEG;
  const sinB =
    Math.sin(dec) * Math.sin(poleDec) +
    Math.cos(dec) * Math.cos(poleDec) * Math.cos(ra - poleRa);
  const b = Math.asin(Math.min(1, Math.max(-1, sinB)));
  const y = Math.cos(dec) * Math.sin(ra - poleRa);
  const x = Math.sin(dec) * Math.cos(poleDec) - Math.cos(dec) * Math.sin(poleDec) * Math.cos(ra - poleRa);
  const l = GALACTIC_LON_OF_NCP - Math.atan2(y, x) * RAD;
  return [wrap360(l), b * RAD];
}

/** Galactic longitude and latitude to ICRS right ascension and declination. */
export function galacticToEquatorial(lDeg: number, bDeg: number): LonLat {
  const l = lDeg * DEG;
  const b = bDeg * DEG;
  const poleDec = GALACTIC_POLE_DEC * DEG;
  const dl = GALACTIC_LON_OF_NCP * DEG - l;
  const sinDec = Math.sin(b) * Math.sin(poleDec) + Math.cos(b) * Math.cos(poleDec) * Math.cos(dl);
  const dec = Math.asin(Math.min(1, Math.max(-1, sinDec)));
  const y = Math.cos(b) * Math.sin(dl);
  const x = Math.sin(b) * Math.cos(poleDec) - Math.cos(b) * Math.sin(poleDec) * Math.cos(dl);
  const ra = GALACTIC_POLE_RA + Math.atan2(y, x) * RAD;
  return [wrap360(ra), dec * RAD];
}

/** An ICRS position expressed in the frame the map is drawn in. */
export function toFrame(raDeg: number, decDeg: number, frame: SkyFrame): LonLat {
  return frame === 'galactic' ? equatorialToGalactic(raDeg, decDeg) : [raDeg, decDeg];
}

/** The longitude the map is centred on in each frame. */
export function frameCenter(frame: SkyFrame): number {
  return frame === 'galactic' ? 0 : 180;
}

const OBLIQUITY = 23.4393;

/** Points along the ecliptic, in ICRS degrees. */
export function eclipticPoints(stepDeg = 2): LonLat[] {
  const points: LonLat[] = [];
  const eps = OBLIQUITY * DEG;
  for (let lambda = 0; lambda <= 360; lambda += stepDeg) {
    const lam = lambda * DEG;
    const ra = Math.atan2(Math.sin(lam) * Math.cos(eps), Math.cos(lam)) * RAD;
    const dec = Math.asin(Math.sin(lam) * Math.sin(eps)) * RAD;
    points.push([wrap360(ra), dec]);
  }
  return points;
}

/** Points along the galactic equator, in ICRS degrees. */
export function galacticEquatorPoints(stepDeg = 2): LonLat[] {
  const points: LonLat[] = [];
  for (let l = 0; l <= 360; l += stepDeg) {
    points.push(galacticToEquatorial(l, 0));
  }
  return points;
}

/** Points along the celestial equator, in ICRS degrees. */
export function celestialEquatorPoints(stepDeg = 2): LonLat[] {
  const points: LonLat[] = [];
  for (let ra = 0; ra <= 360; ra += stepDeg) {
    points.push([ra % 360, 0]);
  }
  return points;
}

/**
 * Quads that tile a band of galactic latitude, as ICRS polygons. Drawn in
 * small pieces so the band can cross the projection's seam without a
 * polygon wrapping the whole map.
 */
export function galacticBandQuads(halfWidthDeg: number, stepDeg = 3): LonLat[][] {
  const quads: LonLat[][] = [];
  for (let l = 0; l < 360; l += stepDeg) {
    quads.push([
      galacticToEquatorial(l, halfWidthDeg),
      galacticToEquatorial(l + stepDeg, halfWidthDeg),
      galacticToEquatorial(l + stepDeg, -halfWidthDeg),
      galacticToEquatorial(l, -halfWidthDeg),
    ]);
  }
  return quads;
}

/**
 * Meridians and parallels of the drawn frame, as polylines in that frame.
 * Parallels start just past the seam so they never jump across it.
 */
export function graticule(centerLonDeg: number): { meridians: LonLat[][]; parallels: LonLat[][] } {
  const meridians: LonLat[][] = [];
  for (let lon = 0; lon < 360; lon += 30) {
    const line: LonLat[] = [];
    for (let lat = -90; lat <= 90; lat += 3) {
      line.push([lon, lat]);
    }
    meridians.push(line);
  }
  const parallels: LonLat[][] = [];
  for (let lat = -60; lat <= 60; lat += 30) {
    const line: LonLat[] = [];
    for (let d = -179.9; d <= 179.9; d += 3) {
      line.push([wrap360(centerLonDeg + d), lat]);
    }
    line.push([wrap360(centerLonDeg + 179.9), lat]);
    parallels.push(line);
  }
  return { meridians, parallels };
}

export interface FootprintShape {
  width_deg: number;
  height_deg: number;
  rotation_deg?: number | null;
  vertices?: [number, number][] | null;
}

/**
 * The outline of one target's field on the sky, in ICRS degrees. A solved
 * footprint is used as it came; a header footprint is a rectangle of the
 * given size about the target, turned by the planned rotation, east of
 * north. Edges are sampled so they bend with the projection.
 */
export function footprintOutline(
  raDeg: number,
  decDeg: number,
  footprint: FootprintShape,
  samplesPerEdge = 6
): LonLat[] {
  const corners: LonLat[] =
    footprint.vertices && footprint.vertices.length >= 3
      ? footprint.vertices.map(([ra, dec]) => [wrap360(ra), dec] as LonLat)
      : rectangleCorners(raDeg, decDeg, footprint);
  const outline: LonLat[] = [];
  for (let i = 0; i < corners.length; i += 1) {
    const [ra0, dec0] = corners[i];
    const [ra1, dec1] = corners[(i + 1) % corners.length];
    const dRa = wrap180(ra1 - ra0);
    for (let s = 0; s < samplesPerEdge; s += 1) {
      const t = s / samplesPerEdge;
      outline.push([wrap360(ra0 + dRa * t), dec0 + (dec1 - dec0) * t]);
    }
  }
  return outline;
}

function rectangleCorners(raDeg: number, decDeg: number, footprint: FootprintShape): LonLat[] {
  const halfW = footprint.width_deg / 2;
  const halfH = footprint.height_deg / 2;
  const theta = (footprint.rotation_deg ?? 0) * DEG;
  const cosT = Math.cos(theta);
  const sinT = Math.sin(theta);
  const cosDec = Math.max(0.02, Math.cos(decDeg * DEG));
  return [
    [-halfW, -halfH],
    [halfW, -halfH],
    [halfW, halfH],
    [-halfW, halfH],
  ].map(([x, y]) => {
    const east = x * cosT - y * sinT;
    const north = x * sinT + y * cosT;
    return [wrap360(raDeg + east / cosDec), Math.max(-89.99, Math.min(89.99, decDeg + north))] as LonLat;
  });
}

/** Whether a set of ICRS outline points straddles the projection seam. */
function crossesSeam(points: SkyPoint[]): boolean {
  for (let i = 1; i < points.length; i += 1) {
    if (Math.abs(points[i].x - points[i - 1].x) > 1) {
      return true;
    }
  }
  return false;
}

export interface PathScale {
  /** Pixel x of projected x = 0. */
  cx: number;
  /** Pixel y of projected y = 0. */
  cy: number;
  /** Pixels per projected unit. */
  scale: number;
}

function toPixel(point: SkyPoint, at: PathScale): string {
  return `${(at.cx + point.x * at.scale).toFixed(1)},${(at.cy - point.y * at.scale).toFixed(1)}`;
}

/**
 * An SVG path for ICRS points drawn in `frame`: projected, scaled, and
 * broken into separate strokes wherever the line would cross the seam.
 */
export function projectedPath(
  icrsPoints: LonLat[],
  frame: SkyFrame,
  at: PathScale,
  close = false
): string {
  const center = frameCenter(frame);
  const projected = icrsPoints.map(([ra, dec]) => {
    const [lon, lat] = toFrame(ra, dec, frame);
    return aitoff(lon, lat, center);
  });
  if (projected.length === 0) {
    return '';
  }
  if (close && crossesSeam(projected)) {
    // A closed shape across the seam would smear across the map. Draw the
    // pieces that lie on one side only.
    return projectedPath(icrsPoints, frame, at, false);
  }
  let path = '';
  let open = false;
  for (let i = 0; i < projected.length; i += 1) {
    const jump = i > 0 && Math.abs(projected[i].x - projected[i - 1].x) > 1;
    if (!open || jump) {
      path += `M${toPixel(projected[i], at)}`;
      open = true;
    } else {
      path += `L${toPixel(projected[i], at)}`;
    }
  }
  return close ? `${path}Z` : path;
}

/** The pixel position of one ICRS point drawn in `frame`. */
export function projectedPoint(raDeg: number, decDeg: number, frame: SkyFrame, at: PathScale): { x: number; y: number } {
  const [lon, lat] = toFrame(raDeg, decDeg, frame);
  const point = aitoff(lon, lat, frameCenter(frame));
  return { x: at.cx + point.x * at.scale, y: at.cy - point.y * at.scale };
}

/** Right ascension in degrees as `12h 34m`. */
export function formatRaShort(raDeg: number): string {
  const hours = wrap360(raDeg) / 15;
  const h = Math.floor(hours);
  const m = Math.round((hours - h) * 60);
  if (m === 60) {
    return `${(h + 1) % 24}h 00m`;
  }
  return `${h}h ${m.toString().padStart(2, '0')}m`;
}

/** Declination in degrees as `+41° 16′`. */
export function formatDecShort(decDeg: number): string {
  const sign = decDeg < 0 ? '−' : '+';
  const abs = Math.abs(decDeg);
  const d = Math.floor(abs);
  const m = Math.round((abs - d) * 60);
  if (m === 60) {
    return `${sign}${d + 1}° 00′`;
  }
  return `${sign}${d}° ${m.toString().padStart(2, '0')}′`;
}

/**
 * Illuminated fraction of the Moon on a civil date, from the mean synodic
 * month counted from the new moon of 2000-01-06. Good to a day or so, which
 * is what a night-by-night strip needs.
 */
export function moonIllumination(unixSeconds: number): number {
  const synodicDays = 29.530588853;
  const newMoon2000 = 947182440; // 2000-01-06 18:14 UTC
  const age = ((((unixSeconds - newMoon2000) / 86400) % synodicDays) + synodicDays) % synodicDays;
  return (1 - Math.cos((age / synodicDays) * 2 * Math.PI)) / 2;
}
