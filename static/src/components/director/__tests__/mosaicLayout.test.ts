import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { DirectorFramingPreview, DirectorFramingRequest, DirectorLayout, DirectorMeasuredGeometry } from '../../../api/directorTypes';
import { cameraAngleAt, framingBoxes, framingGeometry, framingShapes, northTurn, offsetFrom } from '../framingModel';
import {
  asGrid, cameraStep, cameraToPlane, extendFrom, extendMosaic, fillHoles, freshIssues, gridLayout, layoutIssues, layoutStructureProblem, movePanel, nextPanelId,
  planeToCamera, removePanel, ringAround, samePlaces, setLocked, singleLayout, snapPlacement, snapTurn, turnPanel, unitShape, wrapTurn,
} from '../mosaicLayout';
import { boundsOf, intersection, rectangleShape, shapeIssues, shapeOverlap, sharedStrips, turnedShape } from '../panelShape';

/** Written by `crates/director-core/tests/framing_fixture.rs`: a locked framed
 *  target with panels added around it, one dragged short of the minimum. */
const free = JSON.parse(readFileSync(resolve(__dirname, '../../../../../crates/director-core/tests/fixtures/framing-free-layout.json'), 'utf8')) as
  { request: DirectorFramingRequest; preview: DirectorFramingPreview };
/** The same target measured from twelve solved frames shot with a wider
 *  camera, beside planned panels at the current camera's size, one of them
 *  turned by 6°. */
const shot = JSON.parse(readFileSync(resolve(__dirname, '../../../../../crates/director-core/tests/fixtures/framing-measured-layout.json'), 'utf8')) as
  { request: DirectorFramingRequest; preview: DirectorFramingPreview };

const close = (actual: number, expected: number, digits = 9) => expect(actual).toBeCloseTo(expected, digits);
const place = (id: string, x: number, y: number, locked = false) => ({ id, x, y, locked });
const layout = (panels: DirectorLayout['panels'], min = 20): DirectorLayout => ({ min_overlap_percent: min, panels });
const lockedSingle = () => setLocked(singleLayout(20), 'r1c1', true);

describe('the panel model port', () => {
  it('lays out, names and reads back the panels the core does for a free layout', () => {
    const ours = framingGeometry(free.request);
    expect(ours.panels.map(p => [p.id, p.row, p.column, p.locked])).toEqual(free.preview.panels.map(p => [p.id, p.row, p.column, p.locked]));
    for (const [index, panel] of free.preview.panels.entries()) {
      close(ours.panels[index].center.ra_degrees, panel.center.ra_degrees);
      close(ours.panels[index].center.dec_degrees, panel.center.dec_degrees);
    }
    expect(ours.issues).toEqual(free.preview.issues);
    close(ours.extent.width_degrees, free.preview.extent.width_degrees);
    // Each panel's camera angle carries north's lean at its center.
    for (const [index, panel] of free.preview.panels.entries()) close(ours.panels[index].position_angle_degrees, panel.position_angle_degrees);
  });

  it('draws a shot panel as shot and a turned one turned, and reads their overlaps from the outlines as the core does', () => {
    const ours = framingGeometry(shot.request);
    expect(ours.panels.map(p => [p.id, p.row, p.column, p.locked, p.angle_offset_degrees])).toEqual(shot.preview.panels.map(p => [p.id, p.row, p.column, p.locked, p.angle_offset_degrees]));
    for (const [index, panel] of shot.preview.panels.entries()) {
      const mine = ours.panels[index];
      close(mine.center.ra_degrees, panel.center.ra_degrees); close(mine.center.dec_degrees, panel.center.dec_degrees);
      panel.corners.forEach((corner, c) => { close(mine.corners[c].ra_degrees, corner.ra_degrees); close(mine.corners[c].dec_degrees, corner.dec_degrees); });
      panel.view_corners!.forEach((corner, c) => { close(mine.view_corners![c][0], corner[0]); close(mine.view_corners![c][1], corner[1]); });
      close(mine.position_angle_degrees, panel.position_angle_degrees);
    }
    expect(ours.panels[0].measured).toEqual(shot.preview.panels[0].measured);
    expect(ours.issues).toEqual(shot.preview.issues);
    close(ours.extent.width_degrees, shot.preview.extent.width_degrees);
    close(ours.extent.height_degrees, shot.preview.extent.height_degrees);
  });

  it('makes a grid the core reads back as a grid, and numbers added panels on', () => {
    const grid = gridLayout({ rows: 2, columns: 3, overlap_percent: 20 });
    expect(grid.panels.map(p => p.id)).toEqual(['r1c1', 'r1c2', 'r1c3', 'r2c1', 'r2c2', 'r2c3']);
    close(grid.panels[0].x, -0.8); close(grid.panels[0].y, 0.4);
    expect(asGrid(grid)).toEqual({ rows: 2, columns: 3, overlap_percent: 20 });
    expect(nextPanelId(grid)).toBe('p7');
    expect(asGrid(movePanel(grid, 'r1c2', 0.05, 0.4))).toBeNull();
    expect(layoutIssues(grid)).toEqual([]);
  });

  it('refuses only what the core refuses', () => {
    expect(layoutStructureProblem(layout([]))).toMatch(/needs a panel/);
    expect(layoutStructureProblem(layout([place('a', 0, 0), place('a', 1, 0)]))).toMatch(/own id/);
    expect(layoutStructureProblem(layout([place('a', 0, 0), place('b', 0, 0)]))).toMatch(/one spot/);
    expect(layoutStructureProblem(layout([place('r1c1~2', 0, 0)]))).toMatch(/own id/);
    // A gap is the person's call, not a refusal.
    expect(layoutStructureProblem(layout([place('a', 0, 0), place('b', 3, 0)]))).toBeNull();
  });
});

describe('extending a mosaic', () => {
  it('reads N, E, S and W on the sky as the panel side facing nearest that way', () => {
    // At camera angle zero north is up and east is the camera's left.
    expect(cameraStep('N', 0)).toEqual([0, 1]);
    expect(cameraStep('E', 0)).toEqual([-1, 0]);
    expect(cameraStep('NE', 0)).toEqual([-1, 1]);
    expect(cameraStep('SW', 0)).toEqual([1, -1]);
    // Turned a quarter, the camera's up is east and its right is north.
    expect(cameraStep('N', 90)).toEqual([1, 0]);
    expect(cameraStep('E', 90)).toEqual([0, 1]);
    // A little turned, north is still the top side.
    expect(cameraStep('N', 30)).toEqual([0, 1]);
    expect(cameraStep('N', 330)).toEqual([0, 1]);
  });

  it('adds east of a locked framed target at the minimum overlap and leaves it where it is', () => {
    const start = lockedSingle();
    const east = extendFrom(start, 'r1c1', cameraStep('E', 0), 20);
    expect(east.added).toEqual(['p2']);
    expect(east.layout.panels[0]).toEqual(place('r1c1', 0, 0, true));
    close(east.layout.panels[1].x, -0.8); close(east.layout.panels[1].y, 0);
    expect(layoutIssues(east.layout)).toEqual([]);
    // A chosen overlap above the minimum steps less; one below is raised to it.
    close(extendFrom(start, 'r1c1', [0, 1], 30).layout.panels[1].y, 0.7);
    close(extendFrom(start, 'r1c1', [0, 1], 5).layout.panels[1].y, 0.8);
    // A place already taken is said, not doubled.
    expect(extendFrom(east.layout, 'r1c1', [-1, 0], 20)).toMatchObject({ added: [], skipped: ['p2'] });
  });

  it('grows the whole mosaic a row, a column, a corner or a ring at a time', () => {
    const pair = extendFrom(lockedSingle(), 'r1c1', [-1, 0], 20).layout;
    const north = extendMosaic(pair, [0, 1], 20);
    expect(north.added).toEqual(['p3', 'p4']);
    expect(north.layout.panels.filter(p => Math.abs(p.y - 0.8) < 1e-9)).toHaveLength(2);
    expect(layoutIssues(north.layout)).toEqual([]);
    // A diagonal grows both ways and fills the corner: one panel makes a 2 × 2.
    const corner = extendMosaic(lockedSingle(), [-1, 1], 20);
    expect(corner.layout.panels).toHaveLength(4);
    expect(layoutIssues(corner.layout)).toEqual([]);
    // A ring makes the locked target the middle of a 3 × 3.
    const ring = ringAround(lockedSingle(), 20);
    expect(ring.layout.panels).toHaveLength(9);
    expect(ring.layout.panels.find(p => p.id === 'r1c1')).toEqual(place('r1c1', 0, 0, true));
    expect(layoutIssues(ring.layout)).toEqual([]);
  });

  it('fills the holes of a lattice and declines a layout on none', () => {
    const ell = layout([place('r1c1', 0, 0, true), place('p2', -0.8, 0), place('p3', 0, 0.8)]);
    const filled = fillHoles(ell)!;
    expect(filled.added).toEqual(['p4']);
    close(filled.layout.panels[3].x, -0.8); close(filled.layout.panels[3].y, 0.8);
    expect(fillHoles(layout([place('a', 0, 0), place('b', 0.73, 0.1), place('c', 0, 0.8)]))).toBeNull();
  });
});

describe('placing a panel by hand', () => {
  const pair = layout([place('r1c1', 0, 0, true), place('p2', -0.8, 0)]);

  it('settles near the minimum overlap or a lined-up place, and not beyond the reach', () => {
    expect(snapPlacement(pair, 'p2', -0.78, 0.03, 0.05)).toEqual({ x: -0.8, y: 0, snappedX: true, snappedY: true });
    expect(snapPlacement(pair, 'p2', -0.6, 0.2, 0.05)).toEqual({ x: -0.6, y: 0.2, snappedX: false, snappedY: false });
  });

  it('names a thin overlap and a gap, and shades what two panels share', () => {
    const strips = (of: DirectorLayout) => sharedStrips(of.panels.map(unitShape), 'p2', of.min_overlap_percent);
    const thin = movePanel(pair, 'p2', -0.92, 0);
    expect(layoutIssues(thin)).toEqual([{ kind: 'thin_overlap', panels: ['r1c1', 'p2'], percent: 8 }]);
    expect(strips(thin)).toHaveLength(1);
    expect(strips(thin)[0]).toMatchObject({ other: 'r1c1', percent: 8, belowMinimum: true });
    const box = boundsOf(strips(thin)[0].polygon);
    close(box[1] - box[0], 0.08); close(box[3] - box[2], 1);
    const gap = movePanel(pair, 'p2', -1.1, 0);
    expect(layoutIssues(gap)).toEqual([{ kind: 'gap', panels: ['r1c1', 'p2'], percent: 10 }]);
    expect(strips(gap)).toEqual([]);
    expect(strips(pair)[0]).toMatchObject({ percent: 20, belowMinimum: false });
  });

  it('never moves or removes a locked panel', () => {
    expect(movePanel(pair, 'r1c1', 0.3, 0)).toEqual(pair);
    expect(removePanel(pair, 'r1c1')).toEqual(pair);
    expect(removePanel(pair, 'p2').panels.map(p => p.id)).toEqual(['r1c1']);
    expect(removePanel(singleLayout(), 'r1c1').panels).toHaveLength(1);
  });

  it('reads overlap in the camera frame, so turning the mosaic changes none', () => {
    const thin = movePanel(pair, 'p2', -0.92, 0);
    const center = { ra_degrees: 38.2, dec_degrees: 61.45 };
    const size = { width_degrees: 2, height_degrees: 1.5 };
    for (const angle of [0, 17, 90, 133, 271]) {
      const preview = framingGeometry({ center, position_angle_degrees: angle, panel: size, layout: thin, overlays: [], view: null });
      expect(preview.issues).toEqual([{ kind: 'thin_overlap', panels: ['r1c1', 'p2'], percent: 8 }]);
      // On the sky, the two rectangles share 8% of a width along the camera's axis.
      const right = (corner: { ra_degrees: number; dec_degrees: number }) => planeToCamera(offsetFrom(center, corner)!, size, angle)[0];
      const firstLeft = Math.min(...preview.panels[0].corners.map(right));
      const secondRight = Math.max(...preview.panels[1].corners.map(right));
      close(secondRight - firstLeft, 0.08, 6);
      // And the camera frame and the plane are each other's inverse.
      const there = cameraToPlane(-0.92, 0.3, size, angle);
      const back = planeToCamera(there, size, angle);
      close(back[0], -0.92); close(back[1], 0.3);
    }
  });
});

describe('panel outlines', () => {
  it('clip two squares to their shared strip, whichever way their corners run', () => {
    const a = rectangleShape('a', [0, 0], 1, 1);
    const b = rectangleShape('b', [0.8, 0], 1, 1);
    const shared = intersection(a.corners, b.corners);
    const box = boundsOf(shared);
    close(box[1] - box[0], 0.2); close(box[3] - box[2], 1);
    expect(intersection(a.corners, [...b.corners].reverse())).toHaveLength(shared.length);
    expect(intersection(a.corners, rectangleShape('c', [3, 0], 1, 1).corners)).toEqual([]);
  });

  it('read panels of one size as before, and a larger shot panel by its larger side', () => {
    const a = rectangleShape('a', [0, 0], 1, 1);
    for (const [x, y, thickness, run] of [[0.8, 0, 0.2, 1], [0.9, 0.1, 0.1, 0.9], [0.8, 0.5, 0.2, 0.5], [1.25, 0, -0.25, 1]]) {
      const pair = shapeOverlap(a, rectangleShape('b', [x, y], 1, 1))!;
      close(pair.thickness, thickness); close(pair.run, run);
    }
    // A shot panel 2° by 1.4° and a new one 1.8° by 1.2° sharing 0.4°: 20% of the larger.
    const shotPanel = rectangleShape('r1c1', [0, 0], 2, 1.4);
    const fresh = rectangleShape('p2', [1 + 0.9 - 0.4, 0], 1.8, 1.2);
    close(shapeOverlap(shotPanel, fresh)!.thickness, 0.2);
    expect(shapeIssues([shotPanel, fresh], 20)).toEqual([]);
    expect(shapeIssues([shotPanel, rectangleShape('p2', [1 + 0.9 - 0.3, 0], 1.8, 1.2)], 20)).toEqual([{ kind: 'thin_overlap', panels: ['r1c1', 'p2'], percent: 15 }]);
  });

  it('reads a turned panel by its true outline: the strip it shares is its mean width', () => {
    const turned = turnedShape('r1c1', [0, 0], 1, 1, 2);
    const pair = shapeOverlap(turned, rectangleShape('p2', [0.8, 0], 1, 1))!;
    const rad = (2 * Math.PI) / 180;
    const reach = 0.5 * (Math.cos(rad) + Math.sin(rad));
    expect(Math.abs(pair.thickness - (0.5 / Math.cos(rad) - 0.3) / (2 * reach))).toBeLessThan(0.005);
    // A gap is one only where no panel covers it.
    const left = rectangleShape('a', [-0.8, 0], 1, 1); const right = rectangleShape('b', [0.8, 0], 1, 1);
    expect(shapeIssues([left, rectangleShape('m', [0, 0], 1, 1), right], 20)).toEqual([]);
    expect(shapeIssues([left, right], 20)).toEqual([{ kind: 'gap', panels: ['a', 'b'], percent: 60 }]);
  });
});

describe('shot and turned panels', () => {
  const center = { ra_degrees: 38.2, dec_degrees: 61.45 };
  const size = { width_degrees: 2, height_degrees: 1.5 };
  /** The framed target, shot with a camera 2.3° by 1.6° at the plan's angle. */
  const measured: DirectorMeasuredGeometry = { center, position_angle_degrees: 35, size: { width_degrees: 2.3, height_degrees: 1.6 }, source: 'solved', size_source: 'solved', frames: 12 };
  const withShot = layout([{ id: 'r1c1', x: 0, y: 0, locked: true, measured }]);

  it('adds a panel against a shot panel\'s real edges, so it shares the minimum of the wider side', () => {
    const boxes = framingBoxes(center, 35, size, withShot);
    const added = extendFrom(withShot, 'r1c1', [-1, 0], 20, boxes);
    expect(added.added).toEqual(['p2']);
    const shapes = framingShapes(center, 35, size, added.layout);
    close(shapeOverlap(shapes[0], shapes[1])!.thickness, 0.2, 3);
    expect(shapeIssues(shapes, 20)).toEqual([]);
    // Stepped from its planned place instead, the new panel would share a
    // quarter of the wider side, not the fifth asked for.
    const blind = framingShapes(center, 35, size, extendFrom(withShot, 'r1c1', [-1, 0], 20).layout);
    expect(shapeOverlap(blind[0], blind[1])!.thickness).toBeGreaterThan(0.22);
  });

  it('draws a new panel in until it shares the minimum with a shot panel turned off the axes', () => {
    const turned = layout([{ id: 'r1c1', x: 0, y: 0, locked: true, measured: { ...measured, position_angle_degrees: 37 } }]);
    const overlapOf = (of: DirectorLayout, a: string, b: string) => {
      const shapes = framingShapes(center, 35, size, of);
      return shapeOverlap(shapes.find(x => x.id === a)!, shapes.find(x => x.id === b)!)?.thickness ?? null;
    };
    const boxes = framingBoxes(center, 35, size, turned);
    // By its reach alone it would share a little less than the minimum.
    expect(overlapOf(extendFrom(turned, 'r1c1', [-1, 0], 20, boxes).layout, 'r1c1', 'p2')!).toBeLessThan(0.2);
    const fitted = extendFrom(turned, 'r1c1', [-1, 0], 20, boxes, overlapOf).layout;
    const share = overlapOf(fitted, 'r1c1', 'p2')!;
    expect(share).toBeGreaterThanOrEqual(0.2 - 1e-9); expect(share).toBeLessThan(0.2005);
    expect(shapeIssues(framingShapes(center, 35, size, fitted), 20)).toEqual([]);
  });

  it('settles a dragged panel at the minimum beside a shot panel, and a turn on its neighbour\'s', () => {
    const boxes = framingBoxes(center, 35, size, withShot);
    const added = extendFrom(withShot, 'r1c1', [-1, 0], 20, boxes).layout;
    const p2 = added.panels[1];
    const settled = snapPlacement(added, 'p2', p2.x - 0.02, 0.01, 0.05, framingBoxes(center, 35, size, added));
    close(settled.x, p2.x); close(settled.y, 0);
    // A turn settles on 0, on a neighbour's turn, or on how a shot panel leans.
    expect(snapTurn(1.4, [0, 6], 2)).toEqual({ degrees: 0, snapped: true });
    expect(snapTurn(5.1, [0, 6], 2)).toEqual({ degrees: 6, snapped: true });
    expect(snapTurn(12, [0, 6], 2)).toEqual({ degrees: 12, snapped: false });
    expect(snapTurn(179, [-179.5], 2)).toEqual({ degrees: -179.5, snapped: true });
    expect(wrapTurn(190)).toBe(-170); expect(wrapTurn(-180)).toBe(180); expect(wrapTurn(-0)).toBe(0);
  });

  it('turns an unlocked panel, keeps a locked one at the angle it was shot at, and drops the field at no turn', () => {
    const pairOf = layout([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.8, y: 0, locked: false }]);
    const turned = turnPanel(pairOf, 'p2', 12);
    expect(turned.panels[1].angle_offset_degrees).toBe(12);
    expect(samePlaces(pairOf, turned)).toBe(false);
    expect(turnPanel(pairOf, 'r1c1', 12)).toEqual(pairOf);
    expect(turnPanel(turned, 'p2', 0).panels[1]).toEqual({ id: 'p2', x: -0.8, y: 0, locked: false });
    expect(asGrid(turnPanel(gridLayout({ rows: 1, columns: 2, overlap_percent: 20 }), 'r1c2', 3))).toBeNull();
    expect(layoutStructureProblem(layout([{ id: 'a', x: 0, y: 0, locked: false, angle_offset_degrees: 200 }]))).toMatch(/half a turn/);
    // Unlocked, a panel is planned again.
    expect(setLocked(withShot, 'r1c1', false).panels[0].measured).toBeUndefined();
  });

  it('springs a turn back when it leaves the rule short, the same as a drop', () => {
    const pairOf = layout([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.75, y: 0, locked: false }]);
    const issues = (of: DirectorLayout) => shapeIssues(framingShapes(center, 35, size, of), of.min_overlap_percent);
    // Turned 25° the panels share less than the minimum across their seam.
    const far = turnPanel(pairOf, 'p2', 25);
    const fresh = freshIssues(issues(pairOf), issues(far), 'p2');
    expect(fresh.map(issue => issue.kind)).toEqual(['thin_overlap']);
    // A small turn keeps the overlap.
    expect(freshIssues(issues(pairOf), issues(turnPanel(pairOf, 'p2', 1)), 'p2')).toEqual([]);
  });

  it('shoots each planned panel at the framing\'s angle turned by north\'s lean, and its own turn', () => {
    const shotAt = { ra_degrees: 103, dec_degrees: 60 };
    const middle = { ra_degrees: 100, dec_degrees: 60 };
    const lean = northTurn(middle, shotAt);
    expect(Math.abs(lean - 3 * Math.sin((60 * Math.PI) / 180))).toBeLessThan(0.01);
    expect(Math.abs(northTurn(middle, { ra_degrees: 97, dec_degrees: 60 }) + lean)).toBeLessThan(1e-9);
    expect(Math.abs(cameraAngleAt(middle, shotAt, 20) - (20 + lean))).toBeLessThan(0.1);
    const preview = framingGeometry({ center: middle, position_angle_degrees: 20, panel: { width_degrees: 3, height_degrees: 2 }, layout: layout([{ id: 'r1c1', x: 0, y: 0, locked: false, angle_offset_degrees: -15 }, { id: 'p2', x: -0.9, y: 0, locked: false, angle_offset_degrees: 10 }], 10), overlays: [], view: null });
    close(preview.panels[0].position_angle_degrees, 5);
    close(preview.panels[1].position_angle_degrees, cameraAngleAt(middle, preview.panels[1].center, 30));
  });
});
