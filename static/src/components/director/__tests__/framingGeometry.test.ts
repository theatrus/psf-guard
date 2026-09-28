import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { DirectorFramingPreview, DirectorFramingRequest } from '../../../api/directorTypes';
import { deprojectFrom, framingGeometry, offsetFrom } from '../framingModel';

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
