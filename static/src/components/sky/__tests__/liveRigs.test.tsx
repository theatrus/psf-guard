import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { DirectorPlanRow, DirectorRigStatusView, DirectorRigSummary } from '../../../api/directorTypes';
import { placeRigs } from '../liveRigs';
import SkyMap from '../SkyMap';

// As the server sends it: the Director report speaks, exposing while fresh.
const view = (name: string, slug: string, state: DirectorRigStatusView['connectivity']['state'], payload: Record<string, unknown> | null, stale = false): DirectorRigSummary => ({
  rig: { id: `id-${name}`, name, revision: 1 }, catalog_slug: slug, catalog_name: name,
  status: payload ? { rig_id: `id-${name}`, session_id: 's', reported_at_ms: 1_700_000_000_000, received_at_ms: 1_700_000_000_001, payload } : null,
  status_age_ms: 5000, status_stale: stale,
  connectivity: { state, last_contact_ms: 1_700_000_000_001, age_ms: 5000 },
  report: payload ? 'director' : null,
  exposing: !stale && typeof payload?.phase === 'string' && ['exposing', 'imaging', 'capturing'].includes(payload.phase.trim().toLowerCase()),
});
const rig = { id: 'r', name: 'r', revision: 1 };
const target = (name: string, ra: number, dec: number) => ({ name, desired: 10, acquired: 0, accepted: 0, rejected: 0, center: { ra_degrees: ra, dec_degrees: dec }, rotation_degrees: 0 });
const plans: DirectorPlanRow[] = [{
  project: { id: 'p', name: 'M31', revision: 1 }, progress: null, framing: null, plan: null, activation: null,
  links: [
    { catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 1, source_name: 'M31', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [target('M31 panel 1', 10.68, 41.27)] },
    { catalog_slug: 'rc51', catalog_name: 'RC51', rig, source_project_guid: 'g', source_row_id: 1, source_name: 'M31', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [target('Other rig target', 200, -10)] },
  ],
}];
const now = 1_700_000_100_000;

describe('placing rigs on the sky', () => {
  it("prefers the mount's pointing, then the target the rig names among its own plans", () => {
    const [pointing, named, foreign, silent] = placeRigs([
      view('C925', 'c925', 'online', { phase: 'exposing', target_name: 'M31 panel 1', pointing: { ra_degrees: 370, dec_degrees: 12 } }),
      view('C925b', 'c925', 'online', { phase: 'Exposing', target_name: 'm31 PANEL 1' }),
      view('RC51', 'rc51', 'online', { phase: 'slewing', target_name: 'M31 panel 1' }),
      view('Quiet', 'c925', 'never', null),
    ], plans, now);
    expect(pointing).toMatchObject({ source: 'pointing', ra: 10, dec: 12, exposing: true, stale: false });
    expect(named).toMatchObject({ source: 'target', ra: 10.68, dec: 41.27, exposing: true, targetName: 'm31 PANEL 1' });
    // RC51 names a target that only C925's link holds: not placed.
    expect(foreign).toMatchObject({ source: null, ra: null, exposing: false, unplaced: 'No place known for M31 panel 1' });
    expect(silent).toMatchObject({ source: null, ra: null, now: 'No report yet', unplaced: 'No report yet' });
  });

  it('tells a quiet rig from an old report, counts only exposing phases, and ignores a pointing off the sphere', () => {
    const [quiet, stale, bad, waiting] = placeRigs([
      view('A', 'c925', 'stale', { phase: 'exposing', target_name: 'M31 panel 1' }),
      view('B', 'c925', 'online', { phase: 'exposing', target_name: 'M31 panel 1' }, true),
      view('C', 'c925', 'online', { phase: 'idle', pointing: { ra_degrees: 10, dec_degrees: 120 } }),
      view('D', 'c925', 'online', { phase: 'waiting for imaging' }),
    ], plans, now);
    expect(quiet).toMatchObject({ stale: true, staleReason: 'quiet' });
    expect(stale).toMatchObject({ stale: true, staleReason: 'old report', exposing: false });
    expect(bad).toMatchObject({ source: null, ra: null, unplaced: 'Reports no pointing and no target' });
    expect(waiting.exposing).toBe(false);
  });

  it("places a Sync-only rig at the target N.I.N.A.'s sequence is exposing", () => {
    const syncOnly = { ...view('C925', 'c925', 'online', null), report: 'sync' as const, exposing: true };
    syncOnly.sync = { last_contact_ms: now - 5000, plugin_version: '1.4.0', latest_action: null, sequence_stale: false,
      sequence: { session_id: 's', reported_at_ms: now - 5000, received_at_ms: now - 5000, fresh_for_ms: 120_000, running: true,
        activity: { kind: 'exposing', target_name: 'M31 panel 1', exposure_seconds: 300 }, recent: {} } };
    const [placed] = placeRigs([syncOnly], plans, now);
    expect(placed).toMatchObject({ source: 'target', ra: 10.68, dec: 41.27, exposing: true, stale: false, targetName: 'M31 panel 1' });
    expect(placed.now).toMatch(/^Exposing M31 panel 1: 300 s/);
  });

  it('draws placed rigs on the map with their state, and skips the unplaced', () => {
    const rigs = placeRigs([
      view('C925', 'c925', 'online', { phase: 'exposing', target_name: 'M31 panel 1' }),
      view('RC51', 'rc51', 'offline', { phase: 'exposing', pointing: { ra_degrees: 83.8, dec_degrees: -5.4 } }),
      view('Nowhere', 'c925', 'online', { phase: 'idle' }),
    ], plans, now);
    const { container } = render(<SkyMap targets={[]} frame="equatorial" mode="aitoff" showBackdrop={false} showStacks={false} onOpen={() => {}} rigs={rigs} />);
    const drawn = [...container.querySelectorAll('[data-rig]')];
    expect(drawn.map(node => node.getAttribute('data-rig'))).toEqual(['C925', 'RC51']);
    // A place taken from the named target is marked as such; a reported pointing is not.
    expect(drawn[0]).toHaveClass('is-exposing');
    expect(drawn[0]).toHaveClass('is-inferred');
    expect(drawn[0]).toHaveTextContent('C925 · at target');
    expect(drawn[0].querySelector('title')).toHaveTextContent('not a reported position');
    expect(drawn[1]).toHaveClass('is-stale');
    expect(drawn[1]).not.toHaveClass('is-inferred');
    expect(drawn[1]).toHaveTextContent('RC51 (quiet)');
    expect(drawn[1].querySelector('title')).toHaveTextContent('Reported mount position.');
  });
});
