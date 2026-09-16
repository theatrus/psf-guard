import { describe, expect, it } from 'vitest';
import type { SkyCoverage } from '../../../api/types';
import type { WithDb } from '../../../hooks/useDatabases';
import { EVERYTHING, mergeCoverage, shownTargets, skyStats, timelineLanes } from '../skyModel';

function coverage(db_id: string, db_name: string): WithDb<SkyCoverage> {
  return {
    db_id,
    db_name,
    filters: ['L', 'Ha'],
    totals: {
      frames: 4,
      accepted_frames: 3,
      seconds: 1500,
      accepted_seconds: 1200,
      nights: 2,
      first_capture: 1,
      last_capture: 2,
    },
    targets: [
      {
        id: 1,
        name: `${db_name} target`,
        project_id: 1,
        project_name: 'Autumn',
        ra_deg: 10.68,
        dec_deg: 41.27,
        rotation_deg: 0,
        footprint: { width_deg: 2, height_deg: 1.5, source: 'header' },
        frames: 4,
        accepted_frames: 3,
        seconds: 1500,
        accepted_seconds: 1200,
        nights: 2,
        first_capture: 1,
        last_capture: 2,
        filters: [],
      },
      {
        id: 2,
        name: 'Never shot',
        project_id: 1,
        project_name: 'Autumn',
        ra_deg: null,
        dec_deg: null,
        rotation_deg: null,
        footprint: null,
        frames: 0,
        accepted_frames: 0,
        seconds: 0,
        accepted_seconds: 0,
        nights: 0,
        first_capture: null,
        last_capture: null,
        filters: [],
      },
    ],
    nights: [
      { night: '2026-09-08', target_id: 1, filter: 'L', frames: 2, accepted_frames: 1, seconds: 600, accepted_seconds: 300 },
      { night: '2026-09-08', target_id: 1, filter: 'Ha', frames: 1, accepted_frames: 1, seconds: 600, accepted_seconds: 600 },
      { night: '2026-09-09', target_id: 1, filter: 'L', frames: 1, accepted_frames: 1, seconds: 300, accepted_seconds: 300 },
    ],
  };
}

describe('mergeCoverage', () => {
  it('keys targets and nights by database and sorts the nights', () => {
    const merged = mergeCoverage([coverage('b', 'Rig B'), coverage('a', 'Rig A')]);
    expect(merged.targets.map((t) => t.key)).toEqual(['b:1', 'b:2', 'a:1', 'a:2']);
    expect(merged.nightKeys).toEqual(['2026-09-08', '2026-09-09']);
    expect(merged.filters).toEqual(['L', 'Ha']);
    expect(merged.rigs.map((r) => r.db_name)).toEqual(['Rig B', 'Rig A']);
  });
});

describe('shownTargets', () => {
  const merged = mergeCoverage([coverage('a', 'Rig A')]);

  it('leaves out targets with nothing captured and totals the rest', () => {
    const shown = shownTargets(merged, EVERYTHING);
    expect(shown).toHaveLength(1);
    expect(shown[0].seconds).toBe(1500);
    expect(shown[0].frames).toBe(4);
    expect(shown[0].nights).toBe(2);
    expect(shown[0].byFilter).toEqual([
      { filter: 'L', seconds: 900 },
      { filter: 'Ha', seconds: 600 },
    ]);
  });

  it('counts only accepted time when asked', () => {
    const shown = shownTargets(merged, { ...EVERYTHING, acceptedOnly: true });
    expect(shown[0].seconds).toBe(1200);
    expect(shown[0].frames).toBe(3);
  });

  it('stops at the scrubbed night and honours filter and rig cuts', () => {
    const asOf = shownTargets(merged, { ...EVERYTHING, asOfNight: '2026-09-08' });
    expect(asOf[0].seconds).toBe(1200);
    expect(asOf[0].nights).toBe(1);
    const onlyHa = shownTargets(merged, { ...EVERYTHING, filters: new Set(['Ha']) });
    expect(onlyHa[0].seconds).toBe(600);
    expect(shownTargets(merged, { ...EVERYTHING, rigs: new Set(['other']) })).toEqual([]);
  });
});

describe('timelineLanes and skyStats', () => {
  const merged = mergeCoverage([coverage('a', 'Rig A'), coverage('b', 'Rig B')]);

  it('gives each rig a lane with its nights stacked by filter, after a lane that sums them', () => {
    const lanes = timelineLanes(merged, EVERYTHING);
    expect(lanes).toHaveLength(3);
    expect(lanes[0].aggregate).toBe(true);
    expect(lanes[0].seconds).toBe(3000);
    expect(lanes[0].perNight.get('2026-09-08')?.byFilter.get('Ha')).toBe(1200);
    expect(lanes[1].seconds).toBe(1500);
    expect(lanes[1].perNight.get('2026-09-08')?.byFilter.get('Ha')).toBe(600);
    expect(timelineLanes(mergeCoverage([coverage('a', 'Rig A')]), EVERYTHING)).toHaveLength(1);
  });

  it('lists the seconds per night a target was shot, for its sparkline', () => {
    const shown = shownTargets(merged, EVERYTHING);
    expect(shown[0].perNight).toEqual([
      ['2026-09-08', 1200],
      ['2026-09-09', 300],
    ]);
  });

  it('sums the vanity numbers', () => {
    const shown = shownTargets(merged, EVERYTHING);
    const stats = skyStats(shown, timelineLanes(merged, EVERYTHING), EVERYTHING);
    expect(stats.hours).toBeCloseTo(3000 / 3600, 6);
    expect(stats.frames).toBe(8);
    expect(stats.targets).toBe(2);
    expect(stats.nights).toBe(2);
    expect(stats.rigs).toBe(2);
    expect(stats.areaDeg2).toBeCloseTo(6, 6);
    expect(stats.firstNight).toBe('2026-09-08');
    expect(stats.longestNight).toEqual({ night: '2026-09-08', hours: 2400 / 3600 });
    expect(stats.topTarget?.hours).toBeCloseTo(1500 / 3600, 6);
  });
});
