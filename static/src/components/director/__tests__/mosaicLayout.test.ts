import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { DirectorFramingPreview, DirectorFramingRequest, DirectorLayout } from '../../../api/directorTypes';
import { framingGeometry, offsetFrom } from '../framingModel';
import {
  asGrid, cameraStep, cameraToPlane, extendFrom, extendMosaic, fillHoles, gridLayout, layoutIssues, layoutStructureProblem, movePanel, nextPanelId,
  planeToCamera, removePanel, ringAround, setLocked, sharedStrips, singleLayout, snapPlacement,
} from '../mosaicLayout';

/** Written by `crates/director-core/tests/framing_fixture.rs`: a locked framed
 *  target with panels added around it, one dragged short of the minimum. */
const free = JSON.parse(readFileSync(resolve(__dirname, '../../../../../crates/director-core/tests/fixtures/framing-free-layout.json'), 'utf8')) as
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
    const thin = movePanel(pair, 'p2', -0.92, 0);
    expect(layoutIssues(thin)).toEqual([{ kind: 'thin_overlap', panels: ['r1c1', 'p2'], percent: 8 }]);
    const strips = sharedStrips(thin, 'p2');
    expect(strips).toHaveLength(1);
    expect(strips[0]).toMatchObject({ other: 'r1c1', percent: 8, belowMinimum: true });
    const gap = movePanel(pair, 'p2', -1.1, 0);
    expect(layoutIssues(gap)).toEqual([{ kind: 'gap', panels: ['r1c1', 'p2'], percent: 10 }]);
    expect(sharedStrips(gap, 'p2')).toEqual([]);
    expect(sharedStrips(pair, 'p2')[0]).toMatchObject({ percent: 20, belowMinimum: false });
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
