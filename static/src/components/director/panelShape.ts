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
  const ox = (Math.min(ab[1], bb[1]) - Math.max(ab[0], bb[0])) / spanX;
  const oy = (Math.min(ab[3], bb[3]) - Math.max(ab[2], bb[2])) / spanY;
  if (ox <= 0 && oy <= 0) return null;
  // Extents apart along an axis share no area: no clipping needed, and the
  // answer is the same as clipping would give. Most pairs of a large mosaic
  // end here.
  const shared = ox <= 0 || oy <= 0 ? [] : intersection(a.corners, b.corners);
  let meanWidth: [number, number] | null = null;
  let acrossX: number; let acrossY: number;
  if (shared.length === 0) {
    // Outlines that miss each other although their extents overlap on both
    // axes (turned corners passing by): the gap is the distance between the
    // outlines, across the axis it mostly runs along.
    if (ox > 0 && oy > 0) {
      const { distance, direction } = separation(a.corners, b.corners);
      [acrossX, acrossY] = Math.abs(direction[0]) >= Math.abs(direction[1]) ? [-distance / spanX, oy] : [ox, -distance / spanY];
    } else [acrossX, acrossY] = [ox, oy];
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

/** The shortest distance between two convex outlines that do not overlap,
 *  and its direction from `a` toward `b`: the closest of every corner to
 *  every edge, both ways. */
function separation(a: Point[], b: Point[]): { distance: number; direction: Point } {
  let best = { distance: Infinity, direction: [0, 0] as Point };
  const closest = (p: Point, from: Point[], sign: number) => {
    for (let i = 0; i < from.length; i += 1) {
      const s = from[i]; const e = from[(i + 1) % from.length];
      const dx = e[0] - s[0]; const dy = e[1] - s[1];
      const length = dx * dx + dy * dy;
      const t = length > 0 ? Math.min(1, Math.max(0, ((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / length)) : 0;
      const q: Point = [s[0] + t * dx, s[1] + t * dy];
      const distance = Math.hypot(p[0] - q[0], p[1] - q[1]);
      // From the corner's outline toward the other's edge.
      if (distance < best.distance) best = { distance, direction: [sign * (q[0] - p[0]), sign * (q[1] - p[1])] };
    }
  };
  for (const corner of a) closest(corner, b, 1);
  for (const corner of b) closest(corner, a, -1);
  return best;
}

const roundPercent = (share: number) => Math.round(share * 1000) / 10;

function grownContains(shape: PanelShape, point: Point): boolean {
  if (polygonContains(shape.corners, point)) return true;
  const b = boundsOf(shape.corners);
  const tolerance = PLACE_TOLERANCE * Math.max(b[1] - b[0], b[3] - b[2]);
  return [[-1, 0], [1, 0], [0, -1], [0, 1]].some(([dx, dy]) => polygonContains(shape.corners, [point[0] + dx * tolerance, point[1] + dy * tolerance]));
}

/** The panels whose bounds may meet a region, from buckets a panel wide:
 *  never fewer than a scan of every panel finds. */
type NearIndex = (region: Bounds) => number[];
function nearIndex(bounds: Bounds[]): NearIndex {
  const size = Math.max(1e-9, ...bounds.map(b => Math.max(b[1] - b[0], b[3] - b[2])).filter(Number.isFinite));
  const buckets = new Map<string, number[]>();
  bounds.forEach((b, k) => {
    for (let x = Math.floor(b[0] / size); x <= Math.floor(b[1] / size); x += 1) for (let y = Math.floor(b[2] / size); y <= Math.floor(b[3] / size); y += 1) {
      const key = `${x},${y}`; const list = buckets.get(key);
      if (list) list.push(k); else buckets.set(key, [k]);
    }
  });
  const margin = size * PLACE_TOLERANCE * 2;
  return region => {
    const found = new Set<number>();
    for (let x = Math.floor((region[0] - margin) / size); x <= Math.floor((region[1] + margin) / size); x += 1) for (let y = Math.floor((region[2] - margin) / size); y <= Math.floor((region[3] + margin) / size); y += 1) {
      for (const k of buckets.get(`${x},${y}`) ?? []) found.add(k);
    }
    return [...found];
  };
}

function gapCovered(shapes: PanelShape[], bounds: Bounds[], i: number, j: number, pair: ShapeOverlap, index?: NearIndex): boolean {
  const ab = bounds[i]; const bb = bounds[j];
  const [acrossA, acrossB, alongA, alongB]: Array<[number, number]> = pair.verticalSeam
    ? [[ab[0], ab[1]], [bb[0], bb[1]], [ab[2], ab[3]], [bb[2], bb[3]]]
    : [[ab[2], ab[3]], [bb[2], bb[3]], [ab[0], ab[1]], [bb[0], bb[1]]];
  const [low, high] = acrossA[0] + acrossA[1] < acrossB[0] + acrossB[1] ? [acrossA[1], acrossB[0]] : [acrossB[1], acrossA[0]];
  const start = Math.max(alongA[0], alongB[0]); const end = Math.min(alongA[1], alongB[1]);
  // Only a panel reaching into the gap can cover a sample in it, so the rest
  // are not tried at every sample: a large mosaic has hundreds.
  const across: [number, number] = [Math.min(low, high), Math.max(low, high)];
  const along: [number, number] = [Math.min(start, end), Math.max(start, end)];
  const region: Bounds = pair.verticalSeam ? [...across, ...along] : [...along, ...across];
  const near = (index ? index(region) : shapes.map((_, k) => k)).filter(k => {
    const b = bounds[k];
    const t = PLACE_TOLERANCE * Math.max(b[1] - b[0], b[3] - b[2]);
    return k !== i && k !== j && b[0] - t <= region[1] && b[1] + t >= region[0] && b[2] - t <= region[3] && b[3] + t >= region[2];
  }).map(k => shapes[k]);
  for (let s = 0; s < 3; s += 1) {
    const across = low + ((high - low) * (s + 0.5)) / 3;
    for (let t = 0; t < 5; t += 1) {
      const along = start + ((end - start) * (t + 0.5)) / 5;
      const point: Point = pair.verticalSeam ? [across, along] : [along, across];
      if (!near.some(shape => grownContains(shape, point))) return false;
    }
  }
  return true;
}

/** Where the panels fall short of the minimum overlap rule; the same list
 *  the core returns. Shares are of the larger panel's side, so each panel of
 *  a pair shares at least the minimum of its own side. */
export function shapeIssues(shapes: PanelShape[], minOverlapPercent: number): DirectorLayoutIssue[] {
  return readShapes(shapes, minOverlapPercent).issues;
}

/** The rule's reading of a set of panels: what falls short, and which group
 *  of panels joined by at least the minimum overlap each one is in, named by
 *  the group's first member in input order. */
export interface ShapeReading {
  issues: DirectorLayoutIssue[];
  groups: Map<string, string>;
}

export function readShapes(shapes: PanelShape[], minOverlapPercent: number): ShapeReading {
  const tagged = readTagged([shapes], shapes.map(() => 0), [minOverlapPercent]);
  const groups = new Map<string, string>();
  shapes.forEach((shape, k) => groups.set(shape.id, shapes[tagged.groups[k]].id));
  return { issues: tagged.issues.map(issue => ({ ...issue, panels: issue.panels.map(k => shapes[k].id) })), groups };
}

/** Connected groups of a link list, each by its members in input order. */
function groupStarts(linked: number[][]): number[][] {
  const seen = linked.map(() => false);
  const out: number[][] = [];
  for (let start = 0; start < linked.length; start += 1) {
    if (seen[start]) continue;
    const group = [start]; seen[start] = true;
    for (let next = 0; next < group.length; next += 1) for (const other of linked[group[next]]) if (!seen[other]) { seen[other] = true; group.push(other); }
    out.push(group.sort((a, b) => a - b));
  }
  return out;
}

/** An issue read across framings, its panels by their place in the input. */
export interface TaggedIssue { kind: DirectorLayoutIssue['kind']; panels: number[]; percent: number | null }

/** `pair` as shares of the smaller of the two panels' sides, not the
 *  larger's: across the seam for its thickness, along it for its run. The
 *  core's `of_smaller`. */
function ofSmaller(pair: ShapeOverlap, a: Bounds, b: Bounds): ShapeOverlap {
  const ratio = (lo: number, hi: number) => {
    const [sa, sb] = [a[hi] - a[lo], b[hi] - b[lo]];
    const smaller = Math.min(sa, sb);
    return smaller > 0 ? Math.max(sa, sb) / smaller : 1;
  };
  const [across, along] = pair.verticalSeam ? [ratio(0, 1), ratio(2, 3)] : [ratio(2, 3), ratio(0, 1)];
  return { ...pair, thickness: pair.thickness * across, run: Math.min(1, pair.run * along) };
}

/** The rule read over the panels of several framings of one object, as one
 *  cover of the sky: the core's `tagged_issues`. `frames[f]` holds every
 *  panel's outline in framing `f`'s camera frame, all in one order;
 *  `framing[k]` is the framing panel `k` belongs to, and `minimum[f]` its
 *  minimum overlap. Two panels of one framing are read in its frame against
 *  its minimum; two of different framings in the first one's frame against
 *  the larger of the two minimums. With more than one framing, groups apart are
 *  said whatever else is, two panels a thin overlap or a gap keeps apart
 *  counting as one group, and a lone panel as said already. `groups[k]` is
 *  the first member of panel `k`'s group of joined panels. */
export function readTagged(frames: PanelShape[][], framing: number[], minimum: number[]): { issues: TaggedIssue[]; groups: number[] } {
  const n = framing.length;
  const issues: TaggedIssue[] = [];
  const linked: number[][] = framing.map(() => []);
  const bounds = frames.map(shapes => shapes.map(shape => boundsOf(shape.corners)));
  // Built once a frame has a gap to check.
  const indexes = new Map<number, NearIndex>();
  const indexOf = (f: number) => { let index = indexes.get(f); if (!index) { index = nearIndex(bounds[f]); indexes.set(f, index); } return index; };
  const flagged = framing.map(() => false);
  for (let i = 0; i < n; i += 1) {
    const shapes = frames[framing[i]];
    for (let j = i + 1; j < n; j += 1) {
      // Across framings a seam needs the larger of the two minimums.
      const threshold = (framing[i] === framing[j] ? minimum[framing[i]] : Math.max(minimum[framing[i]], minimum[framing[j]])) / 100;
      // Apart on both axes, or a whole panel or more apart on one: nothing to
      // say about the pair. Most pairs of a large cover end here.
      const a = bounds[framing[i]][i]; const b = bounds[framing[i]][j];
      const gapX = Math.max(a[0], b[0]) - Math.min(a[1], b[1]); const gapY = Math.max(a[2], b[2]) - Math.min(a[3], b[3]);
      if ((gapX >= 0 && gapY >= 0) || gapX >= Math.max(a[1] - a[0], b[1] - b[0]) || gapY >= Math.max(a[3] - a[2], b[3] - b[2])) continue;
      const read = shapeOverlap(shapes[i], shapes[j]);
      if (!read) continue;
      // Two rigs' panels may differ in size; a seam between them is read as
      // shares of the smaller one, as a person stitching them would count it.
      const pair = framing[i] === framing[j] ? read : ofSmaller(read, a, b);
      if (pair.thickness >= -PLACE_TOLERANCE && pair.thickness + PLACE_TOLERANCE >= threshold) { linked[i].push(j); linked[j].push(i); continue; }
      if (pair.run + PLACE_TOLERANCE < SIDE_BY_SIDE) continue;
      if (pair.thickness >= -PLACE_TOLERANCE) {
        issues.push({ kind: 'thin_overlap', panels: [i, j], percent: roundPercent(Math.max(0, pair.thickness)) });
        flagged[i] = true; flagged[j] = true;
      } else if (-pair.thickness < 1 && !gapCovered(shapes, bounds[framing[i]], i, j, pair, indexOf(framing[i]))) {
        issues.push({ kind: 'gap', panels: [i, j], percent: roundPercent(-pair.thickness) });
        flagged[i] = true; flagged[j] = true;
      }
    }
  }
  const isolated = framing.map((_, i) => n > 1 && linked[i].length === 0 && !flagged[i]);
  isolated.forEach((alone, i) => { if (alone) issues.push({ kind: 'isolated', panels: [i], percent: null }); });
  const groups = framing.map(() => -1);
  const starts: number[] = [];
  for (let start = 0; start < n; start += 1) {
    if (groups[start] >= 0) continue;
    // A group is named by its first member in input order.
    const group = [start]; groups[start] = start;
    for (let next = 0; next < group.length; next += 1) for (const other of linked[group[next]]) if (groups[other] < 0) { groups[other] = start; group.push(other); }
    starts.push(start);
  }
  if (frames.length > 1) {
    const near = linked.map(list => [...list]);
    for (const issue of issues) if (issue.panels.length === 2) { const [a, b] = issue.panels; near[a].push(b); near[b].push(a); }
    const apart = groupStarts(near).filter(group => !group.every(k => isolated[k])).map(group => group[0]);
    if (apart.length > 1) issues.push({ kind: 'disconnected', panels: apart, percent: null });
  } else if (issues.length === 0 && starts.length > 1) issues.push({ kind: 'disconnected', panels: starts, percent: null });
  return { issues, groups };
}

/** Whether panels that shared a group before no longer do: a move that
 *  splits the mosaic, as opposed to one that only joins groups or fixes the
 *  last issue that hid a split already there. */
export function splitApart(before: Map<string, string>, after: Map<string, string>): boolean {
  const seen = new Map<string, string>();
  for (const [id, group] of before) {
    const now = after.get(id);
    if (now === undefined) continue;
    const other = seen.get(group);
    if (other === undefined) seen.set(group, now);
    else if (other !== now) return true;
  }
  return false;
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
