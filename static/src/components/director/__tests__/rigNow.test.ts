import { describe, expect, it } from 'vitest';
import type { DirectorRigStatusView, SyncLive, SyncSequenceActivity } from '../../../api/directorTypes';
import { describeRigNow, rigIsExposing, rigReportIsStale, rigTargetName } from '../rigNow';

const now = 1_700_000_100_000;
const sync = (activity: SyncSequenceActivity | undefined, { running = true, age = 5000, stale = false } = {}): SyncLive => ({
  last_contact_ms: now - age, plugin_version: '1.4.0', latest_action: null, sequence_stale: stale,
  sequence: { session_id: 's', reported_at_ms: now - age, received_at_ms: now - age, fresh_for_ms: 120_000, running, activity, recent: {} },
});
const view = (phase: string | null, syncLive: SyncLive | null = null, statusStale = false): DirectorRigStatusView => ({
  rig: { id: 'r', name: 'Askar', revision: 1 }, catalog_slug: 'askar', catalog_name: 'Askar', checkins: [],
  status: phase ? { rig_id: 'r', session_id: 's', reported_at_ms: now - 5000, received_at_ms: now - 5000, payload: { phase, target_name: 'M 31' } } : null,
  status_age_ms: 5000, status_stale: statusStale, contacts: { program_pull: null, check_in: null, status: null },
  connectivity: { state: 'online', last_contact_ms: now - 5000, age_ms: 5000 }, assignments: [], pending_receipts: 0, sync: syncLive,
});
const exposing: SyncSequenceActivity = { kind: 'exposing', target_name: 'Propeller Nebula', filter: 'Ha', exposure_seconds: 300, completed: 3, total: 30 };

describe('what a rig is doing, from either plugin', () => {
  it('counts a Sync-only rig as exposing while its sequence report is fresh', () => {
    expect(rigIsExposing(view(null, sync(exposing)), now)).toBe(true);
    expect(rigIsExposing(view(null, sync({ kind: 'autofocus' })), now)).toBe(false);
    expect(rigIsExposing(view(null, sync(exposing, { running: false })), now)).toBe(false);
    expect(rigIsExposing(view(null, sync(exposing, { age: 200_000 })), now)).toBe(false);
    expect(rigIsExposing(view(null, sync(exposing, { stale: true })), now)).toBe(false);
    // The Director plugin's report still counts on its own.
    expect(rigIsExposing(view('exposing'), now)).toBe(true);
    // Either plugin's fresh report is enough.
    expect(rigIsExposing(view('Stopped', sync(exposing)), now)).toBe(true);
  });

  it("states the current plugin's report: Director's while fresh, else Sync's", () => {
    const syncOnly = view(null, sync(exposing));
    expect(describeRigNow(syncOnly, now)).toMatch(/^Exposing Propeller Nebula: 300 s Ha, 3 of 30/);
    expect(rigTargetName(syncOnly, now)).toBe('Propeller Nebula');
    expect(rigReportIsStale(syncOnly, now)).toBe(false);
    const both = view('exposing', sync(exposing));
    expect(describeRigNow(both, now)).toMatch(/^exposing\b.*M 31/);
    expect(rigTargetName(both, now)).toBe('M 31');
    // A stale Director report gives way to a fresh Sync one.
    const staleDirector = view('exposing', sync(exposing), true);
    expect(describeRigNow(staleDirector, now)).toMatch(/^Exposing Propeller Nebula/);
    expect(rigReportIsStale(staleDirector, now)).toBe(false);
    // Neither fresh: the Director report stands, marked stale.
    const bothStale = view('exposing', sync(exposing, { age: 200_000 }), true);
    expect(rigTargetName(bothStale, now)).toBe('M 31');
    expect(rigReportIsStale(bothStale, now)).toBe(true);
    // An old Sync report with no Director report is the rig's, and stale.
    expect(rigReportIsStale(view(null, sync(exposing, { age: 200_000 })), now)).toBe(true);
    expect(describeRigNow(view(null, null), now)).toBe('No status report yet');
  });
});
