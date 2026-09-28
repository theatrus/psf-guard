import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { DirectorFramingPreview, DirectorFramingRequest } from '../../../api/directorTypes';
import { DEFAULT_STAGE, angleAt, deprojectFrom, framingBackdrop, framingGeometry, framingGraticule, gridSteps, handleSky, compassDirections, deprojectOn, offsetFrom, projectOn, stageDeproject, stageFor, stageProject, tileMatrix, toStage, trueWidth, viewAt } from '../framingModel';

/** Written by `crates/director-core/tests/framing_fixture.rs`; the core keeps
 *  reproducing it, so this test pins the browser port to the server. */
const fixture = JSON.parse(readFileSync(resolve(__dirname, '../../../../../crates/director-core/tests/fixtures/framing-preview.json'), 'utf8')) as
  { request: DirectorFramingRequest; preview: DirectorFramingPreview };

const close = (actual: number, expected: number) => expect(Math.abs(actual - expected)).toBeLessThan(1e-9);

describe('framing geometry port', () => {
  it('matches the core on a turned two-by-two mosaic seen from an offset view', () => {
    const ours = framingGeometry(fixture.request);
    const theirs = fixture.preview;
    expect(ours.panels.map(p => p.id)).toEqual(theirs.panels.map(p => p.id));
    close(ours.extent.width_degrees, theirs.extent.width_degrees);
    close(ours.extent.height_degrees, theirs.extent.height_degrees);
    for (const [index, panel] of theirs.panels.entries()) {
      const mine = ours.panels[index];
      expect(mine.row).toBe(panel.row); expect(mine.column).toBe(panel.column);
      close(mine.center.ra_degrees, panel.center.ra_degrees); close(mine.center.dec_degrees, panel.center.dec_degrees);
      panel.corners.forEach((corner, c) => { close(mine.corners[c].ra_degrees, corner.ra_degrees); close(mine.corners[c].dec_degrees, corner.dec_degrees); });
      panel.view_corners!.forEach((corner, c) => { close(mine.view_corners![c][0], corner[0]); close(mine.view_corners![c][1], corner[1]); });
    }
    const overlay = theirs.overlays[0];
    overlay.view_corners!.forEach((corner, c) => { close(ours.overlays[0].view_corners![c][0], corner[0]); close(ours.overlays[0].view_corners![c][1], corner[1]); });
    close(ours.view_center_offset![0], theirs.view_center_offset![0]);
    close(ours.view_center_offset![1], theirs.view_center_offset![1]);
  });

  it('round-trips a position through the plane and drops footprints past its horizon', () => {
    const center = { ra_degrees: 10, dec_degrees: 40 };
    const there = deprojectFrom(center, [1.5, -0.7]);
    const back = offsetFrom(center, there)!;
    close(back[0], 1.5); close(back[1], -0.7);
    const opposite = framingGeometry({ ...fixture.request, view: { center: { ra_degrees: 218.2, dec_degrees: -61.45 }, rotation_degrees: 0 } });
    expect(opposite.panels.every(p => p.view_corners === null)).toBe(true);
    expect(opposite.view_center_offset).toBeNull();
  });
});

describe('the stage projection', () => {
  const view = { ra_degrees: 38.2, dec_degrees: 61.45 };
  it('is stereographic: round trips, matches the tangent plane near the center, and reaches a hemisphere', () => {
    for (const target of [{ ra_degrees: 40, dec_degrees: 62 }, { ra_degrees: 300, dec_degrees: -20 }, { ra_degrees: 38.2, dec_degrees: 89.9 }, { ra_degrees: 218.2, dec_degrees: -50 }]) {
      const offset = stageProject(view, target)!;
      const back = stageDeproject(view, offset);
      close(back.ra_degrees, target.ra_degrees); close(back.dec_degrees, target.dec_degrees);
    }
    const near = { ra_degrees: 39, dec_degrees: 61.8 };
    const tan = offsetFrom(view, near)!; const stg = stageProject(view, near)!;
    expect(Math.abs(tan[0] - stg[0])).toBeLessThan(2e-5); expect(Math.abs(tan[1] - stg[1])).toBeLessThan(2e-5);
    // Past the tangent plane's horizon the stage still has a place for it.
    const far = { ra_degrees: 218.2, dec_degrees: -50 };
    expect(offsetFrom(view, far)).toBeNull();
    expect(stageProject(view, far)).not.toBeNull();
    expect(stageProject(view, { ra_degrees: 218.2, dec_degrees: -61.45 })).toBeNull();
    expect(trueWidth(4)).toBeCloseTo(4, 3);
    expect(trueWidth(150)).toBeLessThan(150); expect(trueWidth(150)).toBeGreaterThan(125);
  });
  it('sizes the stage to the element with the longer side at 1024', () => {
    expect(stageFor(4 / 3)).toEqual(DEFAULT_STAGE);
    expect(stageFor(2)).toEqual({ width: 1024, height: 512 });
    expect(stageFor(0.5)).toEqual({ width: 512, height: 1024 });
    expect(stageFor(NaN)).toEqual(DEFAULT_STAGE);
  });
});

describe('the rotation handle', () => {
  it('stays on the rectangle\'s up direction wherever the view has panned', () => {
    const center = { ra_degrees: 38.2, dec_degrees: 61.45 };
    for (const angle of [0, 35, 125, 250]) {
      const handle = handleSky(center, angle, 0.75, 0.1);
      // Reading the angle back on the target's plane gives the camera angle, whatever the view.
      const wrapped = ((angleAt(center, handle) - angle) % 360 + 360) % 360;
      expect(Math.min(wrapped, 360 - wrapped)).toBeLessThan(1e-6);
      // On the stage, the handle sits where the rectangle's top edge points, from any view center.
      for (const view of [center, { ra_degrees: 50, dec_degrees: 58 }, { ra_degrees: 20, dec_degrees: 70 }]) {
        const geometry = framingGeometry({ center, position_angle_degrees: angle, panel: { width_degrees: 2, height_degrees: 1.5 }, mosaic: { rows: 1, columns: 1, overlap_percent: 0 }, overlays: [], view: { center: view, rotation_degrees: 0 } });
        const [topLeft, , , topRight] = geometry.panels[0].corners;
        const mid = { ra_degrees: (topLeft.ra_degrees + topRight.ra_degrees) / 2, dec_degrees: (topLeft.dec_degrees + topRight.dec_degrees) / 2 };
        const c = toStage(stageProject(view, center)!, 6); const h = toStage(stageProject(view, handle)!, 6); const m = toStage(stageProject(view, mid)!, 6);
        const bearing = (p: number[]) => Math.atan2(p[0] - c[0], -(p[1] - c[1]));
        expect(Math.abs(bearing(h) - bearing(m))).toBeLessThan(0.02);
      }
    }
  });
});

describe('the chart under the framing', () => {
  it('spaces the grid to about five lines and widens right ascension toward the pole', () => {
    // Four degrees across: half-degree parallels and two-minute meridians, which match at the equator.
    expect(gridSteps(4, 0)).toEqual({ raDegrees: 0.5, decDegrees: 0.5 });
    expect(gridSteps(4, 80).raDegrees).toBeGreaterThan(gridSteps(4, 0).raDegrees);
    expect(gridSteps(150, 0)).toEqual({ raDegrees: 15, decDegrees: 20 });
    expect(gridSteps(0.05, 0).decDegrees).toBe(1 / 60);
  });
  it('draws labelled meridians and parallels, and every meridian over the pole', () => {
    const here = { ra_degrees: 38.2, dec_degrees: 61.45 };
    const grid = framingGraticule(viewAt(here, here), 6);
    expect(grid.paths.filter(p => p.kind === 'dec').length).toBeGreaterThanOrEqual(4);
    expect(grid.paths.filter(p => p.kind === 'ra').length).toBeGreaterThanOrEqual(4);
    expect(grid.labels.some(l => l.kind === 'dec' && l.text.startsWith('+61°'))).toBe(true);
    expect(grid.labels.some(l => l.kind === 'ra' && /^2h/.test(l.text))).toBe(true);
    const pole = { ra_degrees: 0, dec_degrees: 89 };
    const polar = framingGraticule(viewAt(pole, pole), 20);
    expect(polar.paths.filter(p => p.kind === 'ra').length).toBeGreaterThanOrEqual(12);
    // Labels never sit on top of one another where meridians crowd.
    const xs = polar.labels.filter(l => l.kind === 'ra').map(l => l.x).sort((a, b) => a - b);
    for (let i = 1; i < xs.length; i += 1) expect(xs[i] - xs[i - 1]).toBeGreaterThanOrEqual(70);
  });
  it('brings in stars, figures, names and the Milky Way as the view widens', () => {
    // Looking at Orion's belt; the constellation's name sits 13° north of it.
    const view = viewAt({ ra_degrees: 84, dec_degrees: 0 }, { ra_degrees: 84, dec_degrees: 0 });
    const narrow = framingBackdrop(view, 4);
    expect(narrow.stars).toHaveLength(0); expect(narrow.figures).toHaveLength(0); expect(narrow.names).toHaveLength(0);
    const wide = framingBackdrop(view, 60);
    expect(wide.stars.length).toBeGreaterThan(20);
    expect(wide.figures.length).toBeGreaterThan(0);
    expect(wide.names.map(n => n.text)).toContain('Orion');
    expect(wide.milkyWay.length).toBeGreaterThan(0);
    expect(framingBackdrop(view, 40).milkyWay).toHaveLength(0);
  });
});

describe('the anchored window', () => {
  const target = { ra_degrees: 38.2, dec_degrees: 61.45 };
  it('keeps a footprint\'s bearing when the window slides over one plane', () => {
    const geometry = framingGeometry({ center: target, position_angle_degrees: 35, panel: { width_degrees: 2, height_degrees: 1.5 }, mosaic: { rows: 1, columns: 1, overlap_percent: 0 }, overlays: [], view: null });
    const corners = geometry.panels[0].corners;
    const bearing = (view: ReturnType<typeof viewAt>) => {
      const [a, b] = corners.map(c => projectOn(view, c)!);
      return Math.atan2(b[1] - a[1], b[0] - a[0]);
    };
    const home = viewAt(target, target);
    const panned = viewAt(target, { ra_degrees: 46, dec_degrees: 63 });
    expect(Math.abs(bearing(home) - bearing(panned))).toBeLessThan(1e-9);
    // The window's own center is where the view was put.
    const back = stageDeproject(panned.anchor, panned.offset);
    expect(back.ra_degrees).toBeCloseTo(46, 6); expect(back.dec_degrees).toBeCloseTo(63, 6);
  });
  it('lays a tile fetched away from the anchor with the turn between the two planes', () => {
    // A tile centered 7.8° of right ascension east of the target: at this
    // declination its north leans about 7° (Δα·sin δ) against the stage's.
    const tile = { center: { ra_degrees: 46, dec_degrees: 61.45 }, fov: 12 };
    const pixels = { width: 2048, height: 1536 };
    const view = viewAt(target, tile.center);
    const matrix = tileMatrix(tile, pixels, view, 6)!;
    const [a, b, c, d] = matrix.slice(7, -1).split(' ').map(Number);
    const turn = (Math.atan2(b, a) * 180) / Math.PI;
    expect(Math.abs(turn)).toBeGreaterThan(6); expect(Math.abs(turn)).toBeLessThan(8);
    // Uniform scale: the tile spans twice the view at 2048 px over a 1024 px stage.
    expect(Math.hypot(a, b)).toBeCloseTo(1, 2); expect(Math.hypot(c, d)).toBeCloseTo(1, 2);
    // A tile at the anchor lies flat: two-to-one with no turn.
    const flat = tileMatrix({ center: target, fov: 12 }, pixels, viewAt(target, target), 6)!;
    const [fa, fb] = flat.slice(7, -1).split(' ').map(Number);
    expect(fb).toBeCloseTo(0, 9); expect(fa).toBeCloseTo(1, 9);
  });
});

describe('a turned sky', () => {
  const target = { ra_degrees: 38.2, dec_degrees: 61.45 };
  it('turns the window by the camera angle and round-trips', () => {
    const view = viewAt(target, target, 35);
    const north = projectOn(view, { ra_degrees: 38.2, dec_degrees: 62.45 })!;
    // North now leans 35° toward the right of the stage (window x runs left).
    expect(Math.atan2(north[0], north[1]) * 180 / Math.PI).toBeCloseTo(-35, 1);
    for (const p of [{ ra_degrees: 40, dec_degrees: 62 }, { ra_degrees: 30, dec_degrees: 58 }]) {
      const back = deprojectOn(view, projectOn(view, p)!);
      expect(back.ra_degrees).toBeCloseTo(p.ra_degrees, 9); expect(back.dec_degrees).toBeCloseTo(p.dec_degrees, 9);
    }
  });
  it('points the compass with the sky', () => {
    const up = compassDirections(0);
    expect(up.north[0]).toBeCloseTo(0, 9); expect(up.north[1]).toBeCloseTo(-1, 9);
    expect(up.east[0]).toBeCloseTo(-1, 9); expect(up.east[1]).toBeCloseTo(0, 9);
    const quarter = compassDirections(90);
    expect(quarter.north[0]).toBeCloseTo(1, 9); expect(quarter.north[1]).toBeCloseTo(0, 9);
    expect(quarter.east[0]).toBeCloseTo(0, 9); expect(quarter.east[1]).toBeCloseTo(-1, 9);
  });
});
