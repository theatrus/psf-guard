import type { DirectorLayout, DirectorLayoutIssue, DirectorMosaic, DirectorOffset, DirectorPanelPlacement, DirectorPanelSize } from '../../api/directorTypes';

/** A mosaic as a set of panels, each with a stable id, its own place and a
 *  lock: the browser's port of `crates/director-core/src/framing/layout.rs`.
 *  Places are on the tangent plane at the framing's center, along the
 *  camera's axes, in panel sizes: `x` toward the camera's right (west at
 *  angle zero), `y` toward its up. Every panel of one framing shares the
 *  panel size and the camera angle, so overlaps are read in the camera's
 *  frame and turning the mosaic never changes one. The fixture the core
 *  writes keeps the two in step. */

export const MAX_PANELS = 256;
export const MAX_PANEL_ID_LEN = 32;
/** Overlap and gap comparisons forgive this much of a panel side. */
export const PLACE_TOLERANCE = 1e-6;
/** Panels sharing at least this much of a side along their seam sit side by side. */
export const SIDE_BY_SIDE = 0.5;
const SAME_PLACE = 1e-6;

/** The panels a rows-by-columns grid makes, centered on the framing: row
 *  one at the top, column one at the camera's left (east at angle zero),
 *  stepped by the unoverlapped share of a panel. */
export function gridLayout(mosaic: DirectorMosaic): DirectorLayout {
  const keep = 1 - mosaic.overlap_percent / 100;
  const panels: DirectorPanelPlacement[] = [];
  for (let row = 0; row < mosaic.rows; row += 1) {
    const y = ((Math.max(0, mosaic.rows - 1)) / 2 - row) * keep;
    for (let column = 0; column < mosaic.columns; column += 1) {
      const x = (column - Math.max(0, mosaic.columns - 1) / 2) * keep;
      panels.push({ id: `r${row + 1}c${column + 1}`, x, y, locked: false });
    }
  }
  return { min_overlap_percent: mosaic.overlap_percent, panels };
}

export function singleLayout(minOverlapPercent = 20): DirectorLayout {
  return gridLayout({ rows: 1, columns: 1, overlap_percent: minOverlapPercent });
}

/** `r{row}c{column}`, one-based, as a grid names its panels. */
export function gridCell(id: string): [number, number] | null {
  const match = /^r([1-9]\d*)c([1-9]\d*)$/.exec(id);
  return match ? [Number(match[1]), Number(match[2])] : null;
}

/** The grid a layout is, when it is one: every cell named as a grid names
 *  it and where the grid at the minimum overlap puts it. Locks do not matter. */
export function asGrid(layout: DirectorLayout): DirectorMosaic | null {
  let rows = 0; let columns = 0;
  for (const panel of layout.panels) {
    const cell = gridCell(panel.id);
    if (!cell) return null;
    rows = Math.max(rows, cell[0]); columns = Math.max(columns, cell[1]);
  }
  if (rows < 1 || columns < 1 || rows > 16 || columns > 16 || rows * columns !== layout.panels.length) return null;
  const mosaic = { rows, columns, overlap_percent: layout.min_overlap_percent };
  const grid = gridLayout(mosaic);
  const fits = grid.panels.every(cell => layout.panels.some(panel => panel.id === cell.id && Math.abs(panel.x - cell.x) <= PLACE_TOLERANCE && Math.abs(panel.y - cell.y) <= PLACE_TOLERANCE));
  return fits ? mosaic : null;
}

export const hasLocked = (layout: DirectorLayout) => layout.panels.some(panel => panel.locked);
export const findPanel = (layout: DirectorLayout, id: string) => layout.panels.find(panel => panel.id === id);

/** A fresh id: `p{n}`, numbered on from the panels there are. */
export function nextPanelId(layout: DirectorLayout): string {
  const highest = Math.max(0, ...layout.panels.map(panel => /^p(\d+)$/.exec(panel.id)).filter(Boolean).map(match => Number(match![1])));
  let n = Math.max(highest, layout.panels.length) + 1;
  while (findPanel(layout, `p${n}`)) n += 1;
  return `p${n}`;
}

const validId = (id: string) => id.length > 0 && id.length <= MAX_PANEL_ID_LEN && /^[A-Za-z0-9_-]+$/.test(id);

/** What the core would refuse in a layout's structure, in a few words; null when sound. */
export function layoutStructureProblem(layout: DirectorLayout): string | null {
  if (layout.panels.length === 0) return 'a mosaic needs a panel';
  if (layout.panels.length > MAX_PANELS) return `at most ${MAX_PANELS} panels`;
  if (!(layout.min_overlap_percent >= 0 && layout.min_overlap_percent <= 90)) return 'minimum overlap 0 to 90%';
  const ids = new Set<string>();
  for (const panel of layout.panels) {
    if (!validId(panel.id) || ids.has(panel.id)) return 'every panel needs its own id';
    ids.add(panel.id);
    if (!Number.isFinite(panel.x) || !Number.isFinite(panel.y)) return 'a panel has no place';
  }
  for (let i = 0; i < layout.panels.length; i += 1) for (let j = i + 1; j < layout.panels.length; j += 1) {
    const a = layout.panels[i]; const b = layout.panels[j];
    if (Math.abs(a.x - b.x) < SAME_PLACE && Math.abs(a.y - b.y) < SAME_PLACE) return `${a.id} and ${b.id} sit on one spot`;
  }
  return null;
}

/** Edge to edge across and up, in panel sizes. */
export function layoutSpan(layout: DirectorLayout): [number, number] {
  if (layout.panels.length === 0) return [0, 0];
  const xs = layout.panels.map(panel => panel.x); const ys = layout.panels.map(panel => panel.y);
  return [Math.max(...xs) - Math.min(...xs) + 1, Math.max(...ys) - Math.min(...ys) + 1];
}

export function layoutExtent(panel: DirectorPanelSize, layout: DirectorLayout): DirectorPanelSize {
  const [across, up] = layoutSpan(layout);
  return { width_degrees: panel.width_degrees * across, height_degrees: panel.height_degrees * up };
}

function bands(values: number[], tolerance: number): number[] {
  const order = values.map((_, index) => index).sort((a, b) => values[a] - values[b]);
  const out = new Array<number>(values.length).fill(0);
  let band = 0; let last: number | null = null;
  for (const index of order) {
    if (last === null || values[index] - last > tolerance) band += 1;
    last = values[index];
    out[index] = band;
  }
  return out;
}

/** Each panel's row and column for listing them: row one furthest up, column one furthest left. */
export function layoutCells(layout: DirectorLayout): Array<[number, number]> {
  const rows = bands(layout.panels.map(panel => -panel.y), 0.1);
  const columns = bands(layout.panels.map(panel => panel.x), 0.1);
  return rows.map((row, index) => [row, columns[index]]);
}

export interface PairOverlap {
  /** Share of the side across the seam the panels overlap; negative for a gap. */
  thickness: number;
  /** Share of the side along the seam they have in common. */
  run: number;
  /** The seam runs up and down the camera frame: the panels sit left and right. */
  verticalSeam: boolean;
}

/** The overlap of two panels at offsets `a` and `b`; null when apart on both axes. */
export function pairOverlap(a: { x: number; y: number }, b: { x: number; y: number }): PairOverlap | null {
  const acrossX = 1 - Math.abs(a.x - b.x);
  const acrossY = 1 - Math.abs(a.y - b.y);
  if (acrossX <= 0 && acrossY <= 0) return null;
  const verticalSeam = acrossX <= acrossY;
  return verticalSeam ? { thickness: acrossX, run: acrossY, verticalSeam } : { thickness: acrossY, run: acrossX, verticalSeam };
}

const roundPercent = (share: number) => Math.round(share * 1000) / 10;

function gapCovered(panels: DirectorPanelPlacement[], i: number, j: number, pair: PairOverlap): boolean {
  const a = panels[i]; const b = panels[j];
  const [acrossA, acrossB, alongA, alongB] = pair.verticalSeam ? [a.x, b.x, a.y, b.y] : [a.y, b.y, a.x, b.x];
  const [low, high] = acrossA < acrossB ? [acrossA + 0.5, acrossB - 0.5] : [acrossB + 0.5, acrossA - 0.5];
  const start = Math.max(alongA, alongB) - 0.5;
  const end = Math.min(alongA, alongB) + 0.5;
  for (let s = 0; s < 3; s += 1) {
    const across = low + ((high - low) * (s + 0.5)) / 3;
    for (let t = 0; t < 5; t += 1) {
      const along = start + ((end - start) * (t + 0.5)) / 5;
      const [px, py] = pair.verticalSeam ? [across, along] : [along, across];
      const covered = panels.some((panel, k) => k !== i && k !== j && Math.abs(px - panel.x) <= 0.5 + PLACE_TOLERANCE && Math.abs(py - panel.y) <= 0.5 + PLACE_TOLERANCE);
      if (!covered) return false;
    }
  }
  return true;
}

/** Where a layout falls short of the minimum overlap rule; the same list the
 *  core returns. Two panels are joined when they overlap by at least the
 *  minimum, side by side or at a corner. Panels side by side (sharing half a
 *  side along their seam) that overlap by less are a thin overlap, and with
 *  uncovered sky between them a gap. A panel joined to nobody is isolated;
 *  groups that never meet are disconnected. */
export function layoutIssues(layout: DirectorLayout): DirectorLayoutIssue[] {
  const minimum = layout.min_overlap_percent / 100;
  const { panels } = layout;
  const n = panels.length;
  const issues: DirectorLayoutIssue[] = [];
  if (n < 2) return issues;
  const linked: number[][] = panels.map(() => []);
  const flagged = panels.map(() => false);
  for (let i = 0; i < n; i += 1) {
    for (let j = i + 1; j < n; j += 1) {
      const pair = pairOverlap(panels[i], panels[j]);
      if (!pair) continue;
      if (pair.thickness >= -PLACE_TOLERANCE && pair.thickness + PLACE_TOLERANCE >= minimum) { linked[i].push(j); linked[j].push(i); continue; }
      if (pair.run + PLACE_TOLERANCE < SIDE_BY_SIDE) continue;
      const ids = [panels[i].id, panels[j].id];
      if (pair.thickness >= -PLACE_TOLERANCE) {
        issues.push({ kind: 'thin_overlap', panels: ids, percent: roundPercent(Math.max(0, pair.thickness)) });
        flagged[i] = true; flagged[j] = true;
      } else if (-pair.thickness < 1 && !gapCovered(panels, i, j, pair)) {
        issues.push({ kind: 'gap', panels: ids, percent: roundPercent(-pair.thickness) });
        flagged[i] = true; flagged[j] = true;
      }
    }
  }
  for (let i = 0; i < n; i += 1) if (linked[i].length === 0 && !flagged[i]) issues.push({ kind: 'isolated', panels: [panels[i].id], percent: null });
  if (issues.length === 0) {
    const seen = panels.map(() => false);
    const starts: string[] = [];
    for (let start = 0; start < n; start += 1) {
      if (seen[start]) continue;
      starts.push(panels[start].id);
      const queue = [start]; seen[start] = true;
      while (queue.length) for (const other of linked[queue.pop()!]) if (!seen[other]) { seen[other] = true; queue.push(other); }
    }
    if (starts.length > 1) issues.push({ kind: 'disconnected', panels: starts, percent: null });
  }
  return issues;
}

/** An issue in a few words, for the panel list and the sky. */
export function describeIssue(issue: DirectorLayoutIssue): string {
  const pair = issue.panels.join(' and ');
  switch (issue.kind) {
    case 'thin_overlap': return `${pair} overlap by only ${issue.percent ?? 0}%`;
    case 'gap': return `${pair} leave a gap of ${issue.percent ?? 0}% of a panel`;
    case 'isolated': return `${pair} overlaps no other panel by the minimum`;
    case 'disconnected': return `the panels fall into ${issue.panels.length} groups that do not overlap`;
  }
}

/** The ids of panels some issue names. */
export function flaggedPanels(issues: DirectorLayoutIssue[]): Set<string> {
  return new Set(issues.filter(issue => issue.kind !== 'disconnected').flatMap(issue => issue.panels));
}

/** Sky directions an extension can take. */
export const DIRECTIONS = ['NW', 'N', 'NE', 'W', 'E', 'SW', 'S', 'SE'] as const;
export type Direction = typeof DIRECTIONS[number];
const DIRECTION_ANGLE: Record<Direction, number> = { N: 0, NE: 45, E: 90, SE: 135, S: 180, SW: 225, W: 270, NW: 315 };
export const DIRECTION_NAMES: Record<Direction, string> = { N: 'north', NE: 'north-east', E: 'east', SE: 'south-east', S: 'south', SW: 'south-west', W: 'west', NW: 'north-west' };

/** The panel side a sky direction leaves by, as a step of -1, 0 or 1 along
 *  the camera's right and up: the side that faces nearest that way at this
 *  camera angle. At angle zero north is up and east is the camera's left. */
export function cameraStep(direction: Direction, positionAngle: number): [number, number] {
  const relative = (((DIRECTION_ANGLE[direction] - positionAngle) % 360) + 360) % 360;
  const diagonal = DIRECTION_ANGLE[direction] % 90 !== 0;
  // The camera side at each quarter turn east of the camera's up.
  const sides: Array<[number, number]> = [[0, 1], [-1, 0], [0, -1], [1, 0]];
  if (!diagonal) return sides[Math.round(relative / 90) % 4];
  const corners: Array<[number, number]> = [[-1, 1], [-1, -1], [1, -1], [1, 1]];
  return corners[Math.round((relative - 45) / 90 + 4) % 4];
}

const taken = (layout: DirectorLayout, x: number, y: number) => layout.panels.find(panel => Math.abs(panel.x - x) < 0.05 && Math.abs(panel.y - y) < 0.05);

export interface Extended { layout: DirectorLayout; added: string[]; skipped: string[] }

/** One panel beyond `fromId` toward `step`, overlapping it by `overlapPercent`. */
export function extendFrom(layout: DirectorLayout, fromId: string, step: [number, number], overlapPercent: number): Extended {
  const from = findPanel(layout, fromId);
  if (!from) return { layout, added: [], skipped: [] };
  const keep = 1 - Math.max(layout.min_overlap_percent, overlapPercent) / 100;
  const x = from.x + step[0] * keep; const y = from.y + step[1] * keep;
  const there = taken(layout, x, y);
  if (there) return { layout, added: [], skipped: [there.id] };
  const id = nextPanelId(layout);
  return { layout: { ...layout, panels: [...layout.panels, { id, x, y, locked: false }] }, added: [id], skipped: [] };
}

/** One more panel at the end of every row (for a step left or right) or
 *  every column (up or down) of the whole mosaic; a diagonal grows both
 *  ways and fills the corner. */
export function extendMosaic(layout: DirectorLayout, step: [number, number], overlapPercent: number): Extended {
  if (step[0] !== 0 && step[1] !== 0) {
    const across = extendMosaic(layout, [step[0], 0], overlapPercent);
    const up = extendMosaic(across.layout, [0, step[1]], overlapPercent);
    return { layout: up.layout, added: [...across.added, ...up.added], skipped: [...across.skipped, ...up.skipped] };
  }
  const horizontal = step[0] !== 0;
  const sign = horizontal ? step[0] : step[1];
  const along = (panel: DirectorPanelPlacement) => horizontal ? panel.y : panel.x;
  const across = (panel: DirectorPanelPlacement) => horizontal ? panel.x : panel.y;
  // Each row (or column) by its place, within a tenth of a panel.
  const lines = bands(layout.panels.map(along), 0.1);
  const ends = new Map<number, DirectorPanelPlacement>();
  layout.panels.forEach((panel, index) => {
    const line = lines[index]; const best = ends.get(line);
    if (!best || across(panel) * sign > across(best) * sign) ends.set(line, panel);
  });
  let current = layout; const added: string[] = []; const skipped: string[] = [];
  for (const end of [...ends.values()].sort((a, b) => along(b) - along(a) || across(a) - across(b))) {
    const next = extendFrom(current, end.id, step, overlapPercent);
    current = next.layout; added.push(...next.added); skipped.push(...next.skipped);
  }
  return { layout: current, added, skipped };
}

/** A ring of panels around the whole mosaic: a column each side, then a
 *  row above and below across them, so a lone framed target becomes the
 *  middle of a 3 by 3. */
export function ringAround(layout: DirectorLayout, overlapPercent: number): Extended {
  let result: Extended = { layout, added: [], skipped: [] };
  for (const step of [[-1, 0], [1, 0], [0, 1], [0, -1]] as Array<[number, number]>) {
    const next = extendMosaic(result.layout, step, overlapPercent);
    result = { layout: next.layout, added: [...result.added, ...next.added], skipped: [...result.skipped, ...next.skipped] };
  }
  return result;
}

/** The holes in a mosaic whose panels sit on one lattice at the minimum
 *  overlap, filled to its bounding rectangle. Null when the panels sit on
 *  no common lattice, since there is then no hole to name. */
export function fillHoles(layout: DirectorLayout): Extended | null {
  const keep = 1 - layout.min_overlap_percent / 100;
  const origin = layout.panels.find(panel => panel.locked) ?? layout.panels[0];
  if (!origin || keep <= 0) return null;
  const index = (value: number, from: number) => (value - from) / keep;
  const cells = layout.panels.map(panel => [index(panel.x, origin.x), index(panel.y, origin.y)]);
  if (cells.some(([i, j]) => Math.abs(i - Math.round(i)) > 0.02 || Math.abs(j - Math.round(j)) > 0.02)) return null;
  const is = cells.map(([i]) => Math.round(i)); const js = cells.map(([, j]) => Math.round(j));
  let current = layout; const added: string[] = [];
  for (let j = Math.max(...js); j >= Math.min(...js); j -= 1) {
    for (let i = Math.min(...is); i <= Math.max(...is); i += 1) {
      const x = origin.x + i * keep; const y = origin.y + j * keep;
      if (taken(current, x, y)) continue;
      const id = nextPanelId(current);
      current = { ...current, panels: [...current.panels, { id, x, y, locked: false }] };
      added.push(id);
    }
  }
  return { layout: current, added, skipped: [] };
}

/** Where a dragged panel settles: each axis pulled to a place that lines it
 *  up with another panel or sets it at the minimum overlap beside one,
 *  when within `tolerance` panel sizes of it. */
export function snapPlacement(layout: DirectorLayout, id: string, x: number, y: number, tolerance: number): { x: number; y: number; snappedX: boolean; snappedY: boolean } {
  const keep = 1 - layout.min_overlap_percent / 100;
  const others = layout.panels.filter(panel => panel.id !== id);
  const nearest = (value: number, candidates: number[]) => candidates.reduce<number | null>((best, candidate) => Math.abs(candidate - value) <= tolerance && (best === null || Math.abs(candidate - value) < Math.abs(best - value)) ? candidate : best, null);
  const sx = nearest(x, others.flatMap(panel => [panel.x, panel.x - keep, panel.x + keep]));
  const sy = nearest(y, others.flatMap(panel => [panel.y, panel.y - keep, panel.y + keep]));
  return { x: sx ?? x, y: sy ?? y, snappedX: sx !== null, snappedY: sy !== null };
}

/** The issues a placement of panel `id` takes part in. */
export function issuesFor(layout: DirectorLayout, id: string): DirectorLayoutIssue[] {
  return layoutIssues(layout).filter(issue => issue.kind === 'disconnected' || issue.panels.includes(id));
}

export interface SharedStrip {
  other: string;
  /** The overlap as a share of the side across the seam, as a percentage. */
  percent: number;
  /** The shared rectangle in camera-frame panel sizes: left, right, bottom, top. */
  box: [number, number, number, number];
  belowMinimum: boolean;
}

/** What panel `id` shares with each panel it overlaps, for drawing. */
export function sharedStrips(layout: DirectorLayout, id: string): SharedStrip[] {
  const panel = findPanel(layout, id);
  if (!panel) return [];
  const minimum = layout.min_overlap_percent / 100;
  return layout.panels.filter(other => other.id !== id).flatMap(other => {
    const pair = pairOverlap(panel, other);
    if (!pair || pair.thickness <= 0) return [];
    const box: SharedStrip['box'] = [Math.max(panel.x, other.x) - 0.5, Math.min(panel.x, other.x) + 0.5, Math.max(panel.y, other.y) - 0.5, Math.min(panel.y, other.y) + 0.5];
    return [{ other: other.id, percent: roundPercent(pair.thickness), box, belowMinimum: pair.run + PLACE_TOLERANCE >= SIDE_BY_SIDE && pair.thickness + PLACE_TOLERANCE < minimum }];
  });
}

/** A camera-frame place, in panel sizes, as degrees east and north on the framing's plane. */
export function cameraToPlane(x: number, y: number, panel: DirectorPanelSize, positionAngle: number): DirectorOffset {
  const rad = (positionAngle * Math.PI) / 180;
  const up = [Math.sin(rad), Math.cos(rad)]; const right = [-Math.cos(rad), Math.sin(rad)];
  const dx = x * panel.width_degrees; const dy = y * panel.height_degrees;
  return [dx * right[0] + dy * up[0], dx * right[1] + dy * up[1]];
}

/** Degrees east and north on the framing's plane as a camera-frame place in panel sizes. */
export function planeToCamera(offset: DirectorOffset, panel: DirectorPanelSize, positionAngle: number): [number, number] {
  const rad = (positionAngle * Math.PI) / 180;
  const dx = -offset[0] * Math.cos(rad) + offset[1] * Math.sin(rad);
  const dy = offset[0] * Math.sin(rad) + offset[1] * Math.cos(rad);
  return [dx / panel.width_degrees, dy / panel.height_degrees];
}

export function movePanel(layout: DirectorLayout, id: string, x: number, y: number): DirectorLayout {
  return { ...layout, panels: layout.panels.map(panel => panel.id === id && !panel.locked ? { ...panel, x, y } : panel) };
}

export function setLocked(layout: DirectorLayout, id: string, locked: boolean): DirectorLayout {
  return { ...layout, panels: layout.panels.map(panel => panel.id === id ? { ...panel, locked } : panel) };
}

/** The layout without panel `id`; a locked panel, or the last one, stays. */
export function removePanel(layout: DirectorLayout, id: string): DirectorLayout {
  const panel = findPanel(layout, id);
  if (!panel || panel.locked || layout.panels.length < 2) return layout;
  return { ...layout, panels: layout.panels.filter(other => other.id !== id) };
}

/** Whether two layouts put the same panels in the same places, whatever the order. */
export function samePlaces(a: DirectorLayout, b: DirectorLayout): boolean {
  return a.panels.length === b.panels.length && a.panels.every(panel => {
    const other = findPanel(b, panel.id);
    return !!other && Math.abs(other.x - panel.x) <= 1e-7 && Math.abs(other.y - panel.y) <= 1e-7;
  });
}
