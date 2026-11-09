import { describe, expect, it } from 'vitest';
import type { SyncLive } from '../../../api/directorTypes';
import { describeActivity, describeRecent, describeSequence, describeSyncAction } from '../syncLiveModel';

const NOW = 1_800_000_000_000;
const live = (sequence: Partial<NonNullable<SyncLive['sequence']>> | null, stale = false): SyncLive => ({
  last_contact_ms: NOW - 60_000, plugin_version: '1.5.0.0', latest_action: null, sequence_stale: stale,
  sequence: sequence && { session_id: 's', reported_at_ms: NOW - 10_000, received_at_ms: NOW - 5_000, fresh_for_ms: 45_000, running: true, recent: {}, ...sequence },
});

describe('Sync plugin on Live', () => {
  it('describes what the plugin last did', () => {
    expect(describeSyncAction({ at_ms: 1, action: 'apply', outcome: 'ok', operation: 'merge', summary: { total_updated: 4, total_inserted: 2 } }))
      .toBe("Merged N.I.N.A.'s catalog: 4 updated, 2 added");
    expect(describeSyncAction({ at_ms: 1, action: 'apply', outcome: 'ok', operation: 'merge', summary: {} })).toBe("Merged N.I.N.A.'s catalog: no changes");
    expect(describeSyncAction({ at_ms: 1, action: 'export', outcome: 'ok', operation: 'push_planning', detail: 'export-id' })).toBe('Sent planning to N.I.N.A.');
    expect(describeSyncAction({ at_ms: 1, action: 'image_upload', outcome: 'ok', operation: 'light', detail: 'M31_0001.fits' })).toBe('Uploaded M31_0001.fits');
    expect(describeSyncAction({ at_ms: 1, action: 'image_upload', outcome: 'refused', detail: 'sha256 mismatch' })).toBe('Uploaded an image: refused (sha256 mismatch)');
    expect(describeSyncAction({ at_ms: 1, action: 'something_new', outcome: 'ok' })).toBe('something new');
  });

  it('describes each running instruction', () => {
    expect(describeActivity({ kind: 'exposing', target_name: 'M 31', exposure_seconds: 300, filter: 'L', completed: 3, total: 30 })).toBe('Exposing M 31: 300 s L, 3 of 30');
    expect(describeActivity({ kind: 'exposing' })).toBe('Exposing');
    expect(describeActivity({ kind: 'mount_slew', target_name: 'M 31' })).toBe('Slewing to M 31');
    expect(describeActivity({ kind: 'camera_cooling', target_temperature_c: -10 })).toBe('Cooling camera to -10 °C');
    expect(describeActivity({ kind: 'safety_wait' })).toBe('Waiting until safe');
    expect(describeActivity({ kind: 'telescope_dance', name: 'Dance' })).toBe('Running Dance');
  });

  it('says how long an instruction has run, and marks an old report as stale', () => {
    const running = live({ activity: { kind: 'autofocus', started_ms: NOW - 130_000 } });
    // 120 s by the plugin's clock to its report, then 5 s since it arrived.
    expect(describeSequence(running, NOW)).toBe('Running autofocus for 2 min');
    expect(describeSequence(live({ activity: { kind: 'autofocus' } }, true), NOW)).toBe('Running autofocus (stale, reported 5 s ago)');
    expect(describeSequence(live({ running: false }), NOW)).toBe('Sequence stopped');
    expect(describeSequence(live(null), NOW)).toBeNull();
  });

  it("shows a wait's end on the server clock", () => {
    // The plugin's clock is 4 min behind: its 03:10 is the server's 03:14.
    const until = new Date(2026, 10, 9, 3, 10).getTime();
    const behind = live({ reported_at_ms: NOW - 240_000 - 1_000, received_at_ms: NOW - 1_000, activity: { kind: 'time_wait', until_ms: until } });
    const shown = new Date(until + 240_000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    expect(describeSequence(behind, NOW)).toBe(`Waiting until ${shown}`);
  });

  it('lists recent N.I.N.A. events on the server clock', () => {
    // The report arrived 5 s after the plugin read it, so plugin times shift by 5 s.
    expect(describeRecent(live({ recent: { image_saved_ms: NOW - 125_000, autofocus_ms: NOW - 2_405_000 } }), NOW)).toBe('last frame 2 min ago · autofocus 40 min ago');
    expect(describeRecent(live({}), NOW)).toBeNull();
  });
});
