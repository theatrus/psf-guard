import type { DirectorLayout, DirectorLayoutIssue, DirectorMosaic, DirectorOffset, DirectorPanelPlacement, DirectorPanelSize } from '../../api/directorTypes';
import { boundsOf, rectangleShape, shapeIssues, type Bounds, type PanelShape } from './panelShape';

/** A mosaic as a set of panels, each with a stable id, its own place and a
 *  lock: the browser's port of `crates/director-core/src/framing/layout.rs`.
 *  Places are on the tangent plane at the framing's center, along the
 *  camera's axes, in panel sizes: `x` toward the camera's right (west at
 *  angle zero), `y` toward its up. Planned panels share the panel size; a
 *  panel may be turned on its own, and a locked one measured from its frames
 *  keeps the size and angle it was shot with. Overlaps are read from the
 *  outlines in the camera's frame, so turning the whole mosaic never
 *  changes one. The fixtures the core writes keep the two in step. */

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

/** `next` with each panel someone unlocked by hand in `before` still marked,
 *  so a layout rebuilt by id does not get locked again by its frames. */
export function keepHandUnlocks(next: DirectorLayout, before: DirectorLayout): DirectorLayout {
  const freed = new Set(before.panels.filter(panel => panel.unlocked_by_hand).map(panel => panel.id));
  if (freed.size === 0) return next;
  return { ...next, panels: next.panels.map(panel => freed.has(panel.id) && !panel.locked ? { ...panel, unlocked_by_hand: true } : panel) };
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
 *  it and where the grid at the minimum overlap puts it, none turned or
 *  drawn as shot. Locks do not matter. */
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
  const fits = grid.panels.every(cell => layout.panels.some(panel => panel.id === cell.id && Math.abs(panel.x - cell.x) <= PLACE_TOLERANCE && Math.abs(panel.y - cell.y) <= PLACE_TOLERANCE && turnOf(panel) === 0 && !panel.measured));
  return fits ? mosaic : null;
}

/** A panel's own turn from the framing's angle, 0 when it has none. */
export const turnOf = (panel: DirectorPanelPlacement) => panel.angle_offset_degrees ?? 0;
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
    if (!(turnOf(panel) >= -180 && turnOf(panel) <= 180)) return 'a panel turns at most half a turn either way';
    if (panel.measured && !panel.locked) return 'only a locked panel keeps the place it was shot';
  }
  for (let i = 0; i < layout.panels.length; i += 1) for (let j = i + 1; j < layout.panels.length; j += 1) {
    const a = layout.panels[i]; const b = layout.panels[j];
    if (Math.abs(a.x - b.x) < SAME_PLACE && Math.abs(a.y - b.y) < SAME_PLACE) return `${a.id} and ${b.id} sit on one spot`;
  }
  return null;
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

/** A panel's place as an unturned unit square, in panel sizes. A turn needs
 *  the panel's size to draw (a turned unit square would shear a panel that
 *  is not square), so a framing with a panel size reads turned and shot
 *  panels by their true outlines (`framingShapes`). */
export function unitShape(panel: DirectorPanelPlacement): PanelShape {
  return rectangleShape(panel.id, [panel.x, panel.y], 1, 1);
}

/** Where a layout falls short of the minimum overlap rule, read from each
 *  panel as an unturned unit square at its place: the core's
 *  `Layout::issues`. A framing reads its true outlines. Two panels are
 *  joined when they overlap by at least the minimum, side by side or at a
 *  corner. Panels side by side (sharing half a side along their seam) that
 *  overlap by less are a thin overlap, and with uncovered sky between them a
 *  gap. A panel joined to nobody is isolated; groups that never meet are
 *  disconnected. */
export function layoutIssues(layout: DirectorLayout): DirectorLayoutIssue[] {
  return shapeIssues(layout.panels.map(unitShape), layout.min_overlap_percent);
}

/** An issue in a few words, for the panel list and the sky. Read `across`
 *  framings, a panel is on its own when it touches no other panel. */
export function describeIssue(issue: DirectorLayoutIssue, across = false): string {
  const pair = issue.panels.join(' and ');
  switch (issue.kind) {
    case 'thin_overlap': return `${pair} overlap by only ${issue.percent ?? 0}%`;
    case 'gap': return `${pair} leave a gap of ${issue.percent ?? 0}% of a panel`;
    case 'isolated': return across ? `${pair} touches no other panel` : `${pair} overlaps no other panel by the minimum`;
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

/** How far a panel reaches in the camera frame, in panel sizes: left,
 *  right, bottom and top of its outline. */
export type BoxOf = (panel: DirectorPanelPlacement) => Bounds;
/** The planned place's reach; a framing passes the true outlines, so a
 *  turned panel, or one shot with another camera, is met at its real edges. */
export const unitBox: BoxOf = panel => boundsOf(unitShape(panel).corners);
const middle = (box: Bounds): [number, number] => [(box[0] + box[1]) / 2, (box[2] + box[3]) / 2];

const taken = (layout: DirectorLayout, x: number, y: number, boxOf: BoxOf) => layout.panels.find(panel => {
  const [cx, cy] = middle(boxOf(panel));
  return Math.abs(cx - x) < 0.05 && Math.abs(cy - y) < 0.05;
});

/** How far apart along one axis two panels sit when they overlap by `share`
 *  of the wider of their two sides there: a new panel of `size` beside one
 *  reaching `reach`. For two panels of one size, `1 - share`. */
const beside = (reach: number, size: number, share: number) => reach / 2 + size / 2 - share * Math.max(reach, size);

export interface Extended { layout: DirectorLayout; added: string[]; skipped: string[] }

/** How much two panels of a layout overlap across their seam, as the rule
 *  reads it from their true outlines; null when they do not meet. */
export type OverlapOf = (layout: DirectorLayout, a: string, b: string) => number | null;

/** One panel beyond `fromId` toward `step`, overlapping it by
 *  `overlapPercent` of the wider side: against its real edges, so a panel
 *  shot with another camera gets the overlap the rule asks for. A shot
 *  panel turned off the camera's axes shares a little less than its reach
 *  says; with `overlapOf` the new panel is drawn in until it shares enough. */
export function extendFrom(layout: DirectorLayout, fromId: string, step: [number, number], overlapPercent: number, boxOf: BoxOf = unitBox, overlapOf?: OverlapOf): Extended {
  const from = findPanel(layout, fromId);
  if (!from) return { layout, added: [], skipped: [] };
  const share = Math.max(layout.min_overlap_percent, overlapPercent) / 100;
  const box = boxOf(from);
  const [cx, cy] = middle(box);
  let x = cx + step[0] * beside(box[1] - box[0], 1, share);
  let y = cy + step[1] * beside(box[3] - box[2], 1, share);
  const id = nextPanelId(layout);
  const placed = (px: number, py: number): DirectorLayout => ({ ...layout, panels: [...layout.panels, { id, x: px, y: py, locked: false }] });
  const short = (px: number, py: number) => (overlapOf?.(placed(px, py), fromId, id) ?? share) + 1e-9 < share;
  // Straight out only: a corner's overlap is no seam.
  if (overlapOf && (step[0] === 0) !== (step[1] === 0) && short(x, y)) {
    // Between where it is and the panel's middle, the least step in that shares enough.
    let low = 0; let high = 1;
    const at = (t: number): [number, number] => [x + (cx - x) * t, y + (cy - y) * t];
    for (let i = 0; i < 30; i += 1) { const mid = (low + high) / 2; if (short(...at(mid))) low = mid; else high = mid; }
    [x, y] = at(high);
  }
  const there = taken(layout, x, y, boxOf);
  if (there) return { layout, added: [], skipped: [there.id] };
  return { layout: placed(x, y), added: [id], skipped: [] };
}

/** One more panel at the end of every row (for a step left or right) or
 *  every column (up or down) of the whole mosaic; a diagonal grows both
 *  ways and fills the corner. */
export function extendMosaic(layout: DirectorLayout, step: [number, number], overlapPercent: number, boxOf: BoxOf = unitBox, overlapOf?: OverlapOf): Extended {
  if (step[0] !== 0 && step[1] !== 0) {
    const across = extendMosaic(layout, [step[0], 0], overlapPercent, boxOf, overlapOf);
    const up = extendMosaic(across.layout, [0, step[1]], overlapPercent, boxOf, overlapOf);
    return { layout: up.layout, added: [...across.added, ...up.added], skipped: [...across.skipped, ...up.skipped] };
  }
  const horizontal = step[0] !== 0;
  const sign = horizontal ? step[0] : step[1];
  const along = (panel: DirectorPanelPlacement) => middle(boxOf(panel))[horizontal ? 1 : 0];
  const across = (panel: DirectorPanelPlacement) => middle(boxOf(panel))[horizontal ? 0 : 1];
  // Each row (or column) by its place, within a tenth of a panel.
  const lines = bands(layout.panels.map(along), 0.1);
  const ends = new Map<number, DirectorPanelPlacement>();
  layout.panels.forEach((panel, index) => {
    const line = lines[index]; const best = ends.get(line);
    if (!best || across(panel) * sign > across(best) * sign) ends.set(line, panel);
  });
  let current = layout; const added: string[] = []; const skipped: string[] = [];
  for (const end of [...ends.values()].sort((a, b) => along(b) - along(a) || across(a) - across(b))) {
    const next = extendFrom(current, end.id, step, overlapPercent, boxOf, overlapOf);
    current = next.layout; added.push(...next.added); skipped.push(...next.skipped);
  }
  return { layout: current, added, skipped };
}

/** A ring of panels around the whole mosaic: a column each side, then a
 *  row above and below across them, so a lone framed target becomes the
 *  middle of a 3 by 3. */
export function ringAround(layout: DirectorLayout, overlapPercent: number, boxOf: BoxOf = unitBox, overlapOf?: OverlapOf): Extended {
  let result: Extended = { layout, added: [], skipped: [] };
  for (const step of [[-1, 0], [1, 0], [0, 1], [0, -1]] as Array<[number, number]>) {
    const next = extendMosaic(result.layout, step, overlapPercent, boxOf, overlapOf);
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
      if (taken(current, x, y, unitBox)) continue;
      const id = nextPanelId(current);
      current = { ...current, panels: [...current.panels, { id, x, y, locked: false }] };
      added.push(id);
    }
  }
  return { layout: current, added, skipped: [] };
}

/** Where a dragged panel settles: each axis pulled to a place that lines it
 *  up with another panel or sets it at the minimum overlap beside one,
 *  against that panel's real edges, when within `tolerance` panel sizes. A
 *  turned outline shares less across the seam than its reach says; with
 *  `overlapOf` a place beside a panel is drawn in until it shares the
 *  minimum, as `extendFrom` does. */
export function snapPlacement(layout: DirectorLayout, id: string, x: number, y: number, tolerance: number, boxOf: BoxOf = unitBox, overlapOf?: OverlapOf): { x: number; y: number; snappedX: boolean; snappedY: boolean } {
  const share = layout.min_overlap_percent / 100;
  const self = findPanel(layout, id);
  const own = self ? boxOf({ ...self, x, y }) : ([x - 0.5, x + 0.5, y - 0.5, y + 0.5] as Bounds);
  const width = own[1] - own[0]; const height = own[3] - own[2];
  // Where the outline's middle sits from the panel's place: none for a
  // planned panel, turned or not, past rounding.
  const settle = (d: number) => (Math.abs(d) < 1e-9 ? 0 : d);
  const [ox, oy] = [settle(middle(own)[0] - x), settle(middle(own)[1] - y)];
  const others = layout.panels.filter(panel => panel.id !== id);
  const boxes = others.map(boxOf);
  // A place lined up with a panel, or beside one (`beside` names it).
  type Candidate = { value: number; beside?: number };
  const nearest = (value: number, candidates: Candidate[]) => candidates.reduce<Candidate | null>((best, candidate) => Math.abs(candidate.value - value) <= tolerance && (best === null || Math.abs(candidate.value - value) < Math.abs(best.value - value)) ? candidate : best, null);
  const along = (axis: 0 | 1, size: number, offset: number): Candidate[] => boxes.flatMap((box, index) => {
    const c = middle(box)[axis] - offset;
    const gap = beside(axis === 0 ? box[1] - box[0] : box[3] - box[2], size, share);
    return [{ value: c }, { value: c - gap, beside: index }, { value: c + gap, beside: index }];
  });
  const sx = nearest(x, along(0, width, ox));
  const sy = nearest(y, along(1, height, oy));
  let fx = sx?.value ?? x; let fy = sy?.value ?? y;
  const refine = (axis: 0 | 1, candidate: Candidate | null) => {
    if (!overlapOf || !self || candidate?.beside === undefined) return;
    const other = others[candidate.beside].id;
    const toward = middle(boxes[candidate.beside])[axis] - (axis === 0 ? ox : oy);
    const at = (t: number) => candidate.value + (toward - candidate.value) * t;
    const placed = (t: number): DirectorLayout => ({ ...layout, panels: layout.panels.map(panel => panel.id === id ? { ...panel, x: axis === 0 ? at(t) : fx, y: axis === 1 ? at(t) : fy } : panel) });
    const short = (t: number) => { const overlap = overlapOf(placed(t), id, other); return overlap !== null && overlap + 1e-9 < share; };
    if (!short(0)) return;
    // Between the snapped place and the panel's middle, the least step in that shares enough.
    let low = 0; let high = 1;
    for (let i = 0; i < 30; i += 1) { const mid = (low + high) / 2; if (short(mid)) low = mid; else high = mid; }
    if (axis === 0) fx = at(high); else fy = at(high);
  };
  refine(0, sx);
  refine(1, sy);
  return { x: fx, y: fy, snappedX: sx !== null, snappedY: sy !== null };
}

/** The turn a turned panel settles on: the nearest of `candidates` (the
 *  framing's angle, a neighbour's, a shot panel's) within `tolerance`
 *  degrees, else where the hand left it. */
export function snapTurn(degrees: number, candidates: number[], tolerance: number): { degrees: number; snapped: boolean } {
  let best: number | null = null;
  for (const candidate of candidates) {
    const apart = Math.abs(wrapTurn(candidate - degrees));
    if (apart <= tolerance && (best === null || apart < Math.abs(wrapTurn(best - degrees)))) best = candidate;
  }
  return best === null ? { degrees, snapped: false } : { degrees: wrapTurn(best), snapped: true };
}

/** A turn in (-180, 180], rounded off at a billionth of a degree. */
export function wrapTurn(degrees: number): number {
  const wrapped = ((((degrees + 180) % 360) + 360) % 360) - 180;
  const rounded = Math.round((wrapped === -180 ? 180 : wrapped) * 1e9) / 1e9;
  return Object.is(rounded, -0) ? 0 : rounded;
}

/** The issues a placement of panel `id` takes part in. */
export function issuesFor(issues: DirectorLayoutIssue[], id: string): DirectorLayoutIssue[] {
  return issues.filter(issue => issue.kind === 'disconnected' || issue.panels.includes(id));
}

/** What moving or turning panel `id` leaves short of the rule that the
 *  layout did not have before. A drop or a turn with any springs back
 *  unless Alt asks for it. The panels falling into groups is only said once
 *  no other issue is, so fixing the last one can show it: it counts as new
 *  only when the move `split` panels that were joined. */
export function freshIssues(before: DirectorLayoutIssue[], after: DirectorLayoutIssue[], id: string, split = true): DirectorLayoutIssue[] {
  const key = (issue: DirectorLayoutIssue) => `${issue.kind}:${issue.panels.join(',')}`;
  const had = new Set(before.map(key));
  return issuesFor(after, id).filter(issue => !had.has(key(issue)) && (issue.kind !== 'disconnected' || split));
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

/** Panel `id` turned to `degrees` from the framing's angle; a locked panel
 *  keeps the angle it was shot at. */
export function turnPanel(layout: DirectorLayout, id: string, degrees: number): DirectorLayout {
  const turn = wrapTurn(degrees);
  return { ...layout, panels: layout.panels.map(panel => {
    if (panel.id !== id || panel.locked) return panel;
    const rest = { ...panel };
    delete rest.angle_offset_degrees;
    return turn === 0 ? rest : { ...rest, angle_offset_degrees: turn };
  }) };
}

/** A locked panel holds its shot geometry; one unlocked is planned again. */
export function setLocked(layout: DirectorLayout, id: string, locked: boolean, shot = false): DirectorLayout {
  return { ...layout, panels: layout.panels.map(panel => {
    if (panel.id !== id) return panel;
    const rest = { ...panel, locked };
    delete rest.unlocked_by_hand;
    if (locked) return rest;
    delete rest.measured;
    // A panel with frames stays unlocked: the framing view would lock it again.
    return shot ? { ...rest, unlocked_by_hand: true } : rest;
  }) };
}

/** The layout without panel `id`; a locked panel, or the last one, stays. */
export function removePanel(layout: DirectorLayout, id: string): DirectorLayout {
  const panel = findPanel(layout, id);
  if (!panel || panel.locked || layout.panels.length < 2) return layout;
  return { ...layout, panels: layout.panels.filter(other => other.id !== id) };
}
