import { describe, expect, it } from 'vitest';
import type { DirectorFramingDraft, DirectorMeasuredGeometry, DirectorRigProfileSummary } from '../../../api/directorTypes';
import { lockedOutlines, offsetFrom, stateFromDraft } from '../framingModel';
import { gridLayout } from '../mosaicLayout';

const rig = (id: string, width: number | null): DirectorRigProfileSummary => ({
  rig: { id, name: id, revision: 1 }, catalog_slug: id, catalog_name: id, profile: null,
  field_of_view: width === null ? null : { width_degrees: width, height_degrees: width * 0.7, pixel_scale_arcsec: 2, focal_ratio: 5 },
  default_exposure_seconds: { broadband: 120, narrowband: 300 },
} as DirectorRigProfileSummary);
const rigs = [rig('redcat', 5.38), rig('askar', 1.8), rig('c925', null), rig('own', 0.9)];
const shot = (ra: number, angle: number): DirectorMeasuredGeometry => ({ center: { ra_degrees: ra, dec_degrees: 61.5 }, position_angle_degrees: angle, size: { width_degrees: 5.31, height_degrees: 3.55 }, source: 'solved', size_source: 'solved', frames: 12 });
const draft = (panels: DirectorFramingDraft['layout']['panels'], rigFramings: DirectorFramingDraft['rig_framings'] = []): DirectorFramingDraft => ({
  project_id: 'p', revision: 1, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
  layout: { ...gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panels }, panel_rig_id: 'redcat', panel: { width_degrees: 5.38, height_degrees: 3.77 },
  shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 8, updated_at_ms: 1, rig_framings: rigFramings,
});
const on = ['redcat', 'askar', 'c925', 'own'];

describe('lockedOutlines', () => {
  it('outlines nothing without a locked shot panel', () => {
    const planned = stateFromDraft(draft([{ id: 'r1c1', x: 0, y: 0, locked: false, measured: shot(38.8, 20) }]));
    expect(lockedOutlines(planned, rigs, on, new Set())).toEqual([]);
    const lockedByHand = stateFromDraft(draft([{ id: 'r1c1', x: 0, y: 0, locked: true }]));
    expect(lockedOutlines(lockedByHand, rigs, on, new Set())).toEqual([]);
  });

  it('outlines each rig that is on, knows its field, and neither shot it nor frames on its own', () => {
    const own = { rig_id: 'own', center: null, position_angle_degrees: null, layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel: null };
    const state = stateFromDraft(draft([{ id: 'r1c1', x: 0, y: 0, locked: true, measured: shot(38.8, 20) }], [own]));
    expect(lockedOutlines(state, rigs, on, new Set(['redcat'])).map(outline => outline.rigId)).toEqual(['askar']);
    expect(lockedOutlines(state, rigs, ['redcat'], new Set(['redcat']))).toEqual([]);
  });

  it('sits where the panel at the center was shot, at its angle', () => {
    const state = stateFromDraft(draft([
      { id: 'p2', x: -0.8, y: 0, locked: true, measured: shot(40.0, 50) },
      { id: 'r1c1', x: 0, y: 0, locked: true, measured: shot(38.8, 20) },
    ]));
    const [outline] = lockedOutlines(state, rigs, ['askar'], new Set());
    // Its corners average to the shot center, not the plan's 38.2°.
    const offsets = outline.corners.map(corner => offsetFrom({ ra_degrees: 38.8, dec_degrees: 61.5 }, corner)!);
    expect(Math.abs(offsets.reduce((sum, [x]) => sum + x, 0) / 4)).toBeLessThan(1e-6);
    expect(Math.abs(offsets.reduce((sum, [, y]) => sum + y, 0) / 4)).toBeLessThan(1e-6);
    // Askar's 1.8° side runs along the shot angle, 20° east of north.
    const [a, b] = [offsets[0], offsets[1]];
    const side = Math.hypot(b[0] - a[0], b[1] - a[1]);
    expect([1.8, 1.26].some(length => Math.abs(side - length) < 1e-3)).toBe(true);
  });
});
