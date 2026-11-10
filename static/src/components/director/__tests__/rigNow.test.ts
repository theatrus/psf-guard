import { describe, expect, it } from 'vitest';
import type { DirectorRigSummary, SyncLive, SyncSequenceActivity } from '../../../api/directorTypes';
import { describeRigNow, rigIsExposing, rigReportIsStale, rigTargetName } from '../rigNow';

const now = 1_700_000_100_000;
const sync = (activity: SyncSequenceActivity | undefined, { age = 5000, stale = false } = {}): SyncLive => ({
  last_contact_ms: now - age, plugin_version: '1.4.0', latest_action: null, sequence_stale: stale,
  sequence: { session_id: 's', reported_at_ms: now - age, received_at_ms: now - age, fresh_for_ms: 120_000, running: true, activity, recent: {} },
});
// The server's answer: which report speaks and whether the rig is exposing.
const view = (report: DirectorRigSummary['report'], exposing: boolean, phase: string | null, syncLive: SyncLive | null = null, statusStale = false): DirectorRigSummary => ({
  rig: { id: 'r', name: 'Askar', revision: 1 }, catalog_slug: 'askar', catalog_name: 'Askar',
  status: phase ? { rig_id: 'r', session_id: 's', reported_at_ms: now - 5000, received_at_ms: now - 5000, payload: { phase, target_name: 'M 31' } } : null,
  status_age_ms: 5000, status_stale: statusStale,
  connectivity: { state: 'online', last_contact_ms: now - 5000, age_ms: 5000 }, sync: syncLive, report, exposing,
});
const exposing: SyncSequenceActivity = { kind: 'exposing', target_name: 'Propeller Nebula', filter: 'Ha', exposure_seconds: 300, completed: 3, total: 30 };

describe("what a rig is doing, as the server decides it", () => {
  it('words the Sync report when it speaks, and the Director report otherwise', () => {
    const syncOnly = view('sync', true, null, sync(exposing));
    expect(rigIsExposing(syncOnly, now)).toBe(true);
    expect(describeRigNow(syncOnly, now)).toMatch(/^Exposing Propeller Nebula: 300 s Ha, 3 of 30/);
    expect(rigTargetName(syncOnly)).toBe('Propeller Nebula');
    const director = view('director', true, 'exposing', sync(exposing));
    expect(describeRigNow(director, now)).toMatch(/^exposing\b.*M 31/);
    expect(rigTargetName(director)).toBe('M 31');
    expect(describeRigNow(view(null, false, null), now)).toBe('No status report yet');
  });

  it('drops an exposing answer whose report has gone stale since it arrived', () => {
    // The page kept an answer while the server could not be reached.
    const later = now + 200_000;
    expect(rigIsExposing(view('sync', true, null, sync(exposing)), later)).toBe(false);
    expect(rigReportIsStale(view('sync', true, null, sync(exposing)), later)).toBe(true);
    expect(rigIsExposing(view('director', true, 'exposing', null, true), now)).toBe(false);
    expect(rigIsExposing(view('director', false, 'slewing'), now)).toBe(false);
  });
});
