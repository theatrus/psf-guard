import type { DirectorLayoutIssue } from '../../api/directorTypes';

/** Panels as convex quadrilaterals in the mosaic's camera frame, and the
 *  minimum overlap rule read from their intersections: the browser's port of
 *  `crates/director-core/src/framing/shape.rs`. Planned panels are
 *  rectangles, turned when a panel has a turn of its own; a panel measured
 *  from its frames keeps the size and angle it was shot with. The fixtures
 *  the core writes keep the two in step. */

/** Right and up in the camera frame: degrees on the plane, or panel sizes. */
export type Point = [number, number];
export interface PanelShape { id: string; corners: [Point, Point, Point, Point] }
/** Left, right, bottom and top. */
export type Bounds = [number, number, number, number];

/** Overlap comparisons forgive this much of a side. */
const PLACE_TOLERANCE = 1e-6;
/** Panels sharing at least this much of a side along their seam sit side by side. */
const SIDE_BY_SIDE = 0.5;

export function rectangleShape(id: string, center: Point, width: number, height: number): PanelShape {
  const hw = width / 2; const hh = height / 2;
  return { id, corners: [[center[0] - hw, center[1] + hh], [center[0] - hw, center[1] - hh], [center[0] + hw, center[1] - hh], [center[0] + hw, center[1] + hh]] };
}

/** A rectangle turned counter-clockwise by `degrees` about its center. */
export function turnedShape(id: string, center: Point, width: number, height: number, degrees: number): PanelShape {
  const shape = rectangleShape(id, center, width, height);
  if (degrees === 0) return shape;
  const rad = (degrees * Math.PI) / 180;
  const sin = Math.sin(rad); const cos = Math.cos(rad);
  shape.corners = shape.corners.map(([x, y]) => {
    const dx = x - center[0]; const dy = y - center[1];
    return [center[0] + dx * cos - dy * sin, center[1] + dx * sin + dy * cos];
  }) as PanelShape['corners'];
  return shape;
}

export function boundsOf(points: Point[]): Bounds {
  const out: Bounds = [Infinity, -Infinity, Infinity, -Infinity];
  for (const [x, y] of points) { out[0] = Math.min(out[0], x); out[1] = Math.max(out[1], x); out[2] = Math.min(out[2], y); out[3] = Math.max(out[3], y); }
  return out;
}

export function shapeCenter(shape: PanelShape): Point {
  return [shape.corners.reduce((sum, c) => sum + c[0] / 4, 0), shape.corners.reduce((sum, c) => sum + c[1] / 4, 0)];
}

const cross = (o: Point, a: Point, b: Point) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);

/** Twice the signed area: positive when the corners run counter-clockwise. */
function signedArea(points: Point[]): number {
  let sum = 0;
  for (let i = 0; i < points.length; i += 1) { const a = points[i]; const b = points[(i + 1) % points.length]; sum += a[0] * b[1] - b[0] * a[1]; }
  return sum;
}

/** Inside a convex polygon or on its edge, whichever way its corners run. */
export function polygonContains(polygon: Point[], point: Point): boolean {
  const orientation = Math.sign(signedArea(polygon));
  return polygon.every((corner, i) => cross(corner, polygon[(i + 1) % polygon.length], point) * orientation >= -1e-12);
}

function crossing(p: Point, q: Point, a: Point, b: Point): Point {
  const dpx = q[0] - p[0]; const dpy = q[1] - p[1];
  const dax = b[0] - a[0]; const day = b[1] - a[1];
  const denominator = dpx * day - dpy * dax;
  if (Math.abs(denominator) < 1e-300) return p;
  const t = ((a[0] - p[0]) * day - (a[1] - p[1]) * dax) / denominator;
  return [p[0] + t * dpx, p[1] + t * dpy];
}

/** The intersection of two convex polygons (Sutherland–Hodgman), empty when they do not overlap. */
export function intersection(subject: Point[], clip: Point[]): Point[] {
  const orientation = Math.sign(signedArea(clip));
  let output = [...subject];
  for (let i = 0; i < clip.length && output.length > 0; i += 1) {
    const a = clip[i]; const b = clip[(i + 1) % clip.length];
    const inside = (p: Point) => cross(a, b, p) * orientation >= 0;
    const input = output; output = [];
    for (let j = 0; j < input.length; j += 1) {
      const current = input[j]; const previous = input[(j + input.length - 1) % input.length];
      const currentIn = inside(current); const previousIn = inside(previous);
      if (currentIn) { if (!previousIn) output.push(crossing(previous, current, a, b)); output.push(current); }
      else if (previousIn) output.push(crossing(previous, current, a, b));
    }
  }
  return output.length < 3 || Math.abs(signedArea(output)) < 1e-18 ? [] : output;
}

export interface ShapeOverlap {
  /** Share of the side across the seam, of the larger panel; negative for a gap. */
  thickness: number;
  /** Share of the side along the seam they have in common. */
  run: number;
  /** The seam runs up and down the camera frame: the panels sit left and right. */
  verticalSeam: boolean;
  /** Where they overlap; empty for none. */
  shared: Point[];
}

/** How two panels sit against each other, as the core reads it. */
export function shapeOverlap(a: PanelShape, b: PanelShape): ShapeOverlap | null {
  const ab = boundsOf(a.corners); const bb = boundsOf(b.corners);
  const spanX = Math.max(ab[1] - ab[0], bb[1] - bb[0]);
  const spanY = Math.max(ab[3] - ab[2], bb[3] - bb[2]);
  if (!(spanX > 0 && spanY > 0)) return null;
  const shared = intersection(a.corners, b.corners);
  let meanWidth: [number, number] | null = null;
  let acrossX: number; let acrossY: number;
  if (shared.length === 0) {
    const ox = (Math.min(ab[1], bb[1]) - Math.max(ab[0], bb[0])) / spanX;
    const oy = (Math.min(ab[3], bb[3]) - Math.max(ab[2], bb[2])) / spanY;
    if (ox <= 0 && oy <= 0) return null;
    if (ox > 0 && oy > 0) [acrossX, acrossY] = ox <= oy ? [0, oy] : [ox, 0];
    else [acrossX, acrossY] = [ox, oy];
  } else {
    const sb = boundsOf(shared);
    const area = Math.abs(signedArea(shared)) / 2;
    meanWidth = [area / (sb[3] - sb[2]) / spanX, area / (sb[1] - sb[0]) / spanY];
    [acrossX, acrossY] = [(sb[1] - sb[0]) / spanX, (sb[3] - sb[2]) / spanY];
  }
  const verticalSeam = acrossX <= acrossY;
  const thickness = meanWidth ? (verticalSeam ? meanWidth[0] : meanWidth[1]) : verticalSeam ? acrossX : acrossY;
  return { thickness, run: verticalSeam ? acrossY : acrossX, verticalSeam, shared };
}

const roundPercent = (share: number) => Math.round(share * 1000) / 10;

function grownContains(shape: PanelShape, point: Point): boolean {
  if (polygonContains(shape.corners, point)) return true;
  const b = boundsOf(shape.corners);
  const tolerance = PLACE_TOLERANCE * Math.max(b[1] - b[0], b[3] - b[2]);
  return [[-1, 0], [1, 0], [0, -1], [0, 1]].some(([dx, dy]) => polygonContains(shape.corners, [point[0] + dx * tolerance, point[1] + dy * tolerance]));
}

function gapCovered(shapes: PanelShape[], i: number, j: number, pair: ShapeOverlap): boolean {
  const ab = boundsOf(shapes[i].corners); const bb = boundsOf(shapes[j].corners);
  const [acrossA, acrossB, alongA, alongB]: Array<[number, number]> = pair.verticalSeam
    ? [[ab[0], ab[1]], [bb[0], bb[1]], [ab[2], ab[3]], [bb[2], bb[3]]]
    : [[ab[2], ab[3]], [bb[2], bb[3]], [ab[0], ab[1]], [bb[0], bb[1]]];
  const [low, high] = acrossA[0] + acrossA[1] < acrossB[0] + acrossB[1] ? [acrossA[1], acrossB[0]] : [acrossB[1], acrossA[0]];
  const start = Math.max(alongA[0], alongB[0]); const end = Math.min(alongA[1], alongB[1]);
  for (let s = 0; s < 3; s += 1) {
    const across = low + ((high - low) * (s + 0.5)) / 3;
    for (let t = 0; t < 5; t += 1) {
      const along = start + ((end - start) * (t + 0.5)) / 5;
      const point: Point = pair.verticalSeam ? [across, along] : [along, across];
      if (!shapes.some((shape, k) => k !== i && k !== j && grownContains(shape, point))) return false;
    }
  }
  return true;
}

/** Where the panels fall short of the minimum overlap rule; the same list
 *  the core returns. Shares are of the larger panel's side, so each panel of
 *  a pair shares at least the minimum of its own side. */
export function shapeIssues(shapes: PanelShape[], minOverlapPercent: number): DirectorLayoutIssue[] {
  const minimum = minOverlapPercent / 100;
  const n = shapes.length;
  const issues: DirectorLayoutIssue[] = [];
  if (n < 2) return issues;
  const linked: number[][] = shapes.map(() => []);
  const flagged = shapes.map(() => false);
  for (let i = 0; i < n; i += 1) {
    for (let j = i + 1; j < n; j += 1) {
      const pair = shapeOverlap(shapes[i], shapes[j]);
      if (!pair) continue;
      if (pair.thickness >= -PLACE_TOLERANCE && pair.thickness + PLACE_TOLERANCE >= minimum) { linked[i].push(j); linked[j].push(i); continue; }
      if (pair.run + PLACE_TOLERANCE < SIDE_BY_SIDE) continue;
      const ids = [shapes[i].id, shapes[j].id];
      if (pair.thickness >= -PLACE_TOLERANCE) {
        issues.push({ kind: 'thin_overlap', panels: ids, percent: roundPercent(Math.max(0, pair.thickness)) });
        flagged[i] = true; flagged[j] = true;
      } else if (-pair.thickness < 1 && !gapCovered(shapes, i, j, pair)) {
        issues.push({ kind: 'gap', panels: ids, percent: roundPercent(-pair.thickness) });
        flagged[i] = true; flagged[j] = true;
      }
    }
  }
  for (let i = 0; i < n; i += 1) if (linked[i].length === 0 && !flagged[i]) issues.push({ kind: 'isolated', panels: [shapes[i].id], percent: null });
  if (issues.length === 0) {
    const seen = shapes.map(() => false);
    const starts: string[] = [];
    for (let start = 0; start < n; start += 1) {
      if (seen[start]) continue;
      // A group is named by its first member in input order.
      const group = [start]; seen[start] = true;
      for (let next = 0; next < group.length; next += 1) for (const other of linked[group[next]]) if (!seen[other]) { seen[other] = true; group.push(other); }
      starts.push(shapes[Math.min(...group)].id);
    }
    if (starts.length > 1) issues.push({ kind: 'disconnected', panels: starts, percent: null });
  }
  return issues;
}

export interface SharedStrip {
  other: string;
  /** The overlap as a share of the side across the seam, as a percentage. */
  percent: number;
  /** Where the two overlap, in the shapes' own frame. */
  polygon: Point[];
  belowMinimum: boolean;
}

/** What panel `id` shares with each panel it overlaps, for drawing. */
export function sharedStrips(shapes: PanelShape[], id: string, minOverlapPercent: number): SharedStrip[] {
  const shape = shapes.find(entry => entry.id === id);
  if (!shape) return [];
  const minimum = minOverlapPercent / 100;
  return shapes.filter(other => other.id !== id).flatMap(other => {
    const pair = shapeOverlap(shape, other);
    if (!pair || pair.shared.length === 0 || pair.thickness <= 0) return [];
    return [{ other: other.id, percent: roundPercent(pair.thickness), polygon: pair.shared, belowMinimum: pair.run + PLACE_TOLERANCE >= SIDE_BY_SIDE && pair.thickness + PLACE_TOLERANCE < minimum }];
  });
}
