import { describe, expect, it } from 'vitest';
import type { SkyCoverage } from '../../../api/types';
import type { WithDb } from '../../../hooks/useDatabases';
import { EVERYTHING, coveredSky, formatPixels, mergeCoverage, nightKeysFor, shownTargets, skyStats, timelineLanes } from '../skyModel';

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
      night_starts_utc_seconds: 12 * 3600,
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
        footprint: { width_deg: 2, height_deg: 1.5, source: 'header', pixel_scale_arcsec: 2 },
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
    const from = shownTargets(merged, { ...EVERYTHING, fromNight: '2026-09-09' });
    expect(from[0].seconds).toBe(300);
    expect(from[0].firstNight).toBe('2026-09-09');
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
    // Both rigs' fields sit on the same 2° × 1.5° patch: the sky covered is
    // that patch once, while the fields add up to twice it.
    expect(stats.areaDeg2).toBeGreaterThan(2.85);
    expect(stats.areaDeg2).toBeLessThan(3.15);
    expect(stats.fieldsDeg2).toBeCloseTo(6, 6);
    expect(stats.firstNight).toBe('2026-09-08');
    // The most one rig got in one night, never the rigs added together.
    expect(stats.longestNight).toEqual({ night: '2026-09-08', hours: 1200 / 3600, rig: 'Rig A' });
    expect(stats.topTarget?.hours).toBeCloseTo(1500 / 3600, 6);
  });
});

describe('coveredSky', () => {
  it('counts overlapping fields once and separate fields in full', () => {
    const merged = mergeCoverage([coverage('a', 'Rig A')]);
    const [one] = shownTargets(merged, EVERYTHING);
    const single = coveredSky([one]);
    expect(single.areaDeg2).toBeGreaterThan(2.85);
    expect(single.areaDeg2).toBeLessThan(3.15);
    expect(coveredSky([one, one]).areaDeg2).toBeCloseTo(single.areaDeg2, 6);
    expect(coveredSky([one, one]).fieldsDeg2).toBeCloseTo(6, 6);
    const elsewhere = { ...one, target: { ...one.target, key: 'a:9', ra_deg: 200, dec_deg: -30 } };
    const both = coveredSky([one, elsewhere]);
    expect(both.areaDeg2).toBeGreaterThan(5.7);
    expect(both.areaDeg2).toBeLessThan(6.3);
    const turned = { ...one, target: { ...one.target, footprint: { ...one.target.footprint!, rotation_deg: 45 } } };
    expect(coveredSky([turned]).areaDeg2).toBeGreaterThan(2.85);
  });

  it('turns the covered sky into pixels at the finest scale that reached each patch', () => {
    const merged = mergeCoverage([coverage('a', 'Rig A')]);
    const [one] = shownTargets(merged, EVERYTHING);
    // A 2° × 1.5° field at 2"/px holds 3600 × 2700 pixels.
    const alone = coveredSky([one]).pixels;
    expect(alone).toBeGreaterThan(3600 * 2700 * 0.95);
    expect(alone).toBeLessThan(3600 * 2700 * 1.05);
    // A second rig on the same patch at 0.5"/px resolves it sixteen times finer.
    const finer = { ...one, target: { ...one.target, key: 'b:1', footprint: { ...one.target.footprint!, pixel_scale_arcsec: 0.5 } } };
    const together = coveredSky([one, finer]).pixels;
    expect(together).toBeGreaterThan(alone * 15);
    expect(together).toBeLessThan(alone * 17);
    // A field with no known scale covers sky but counts no pixels.
    const unknown = { ...one, target: { ...one.target, footprint: { ...one.target.footprint!, pixel_scale_arcsec: null } } };
    expect(coveredSky([unknown]).pixels).toBe(0);
    expect(coveredSky([unknown]).areaDeg2).toBeGreaterThan(2.85);
  });

  it('formats pixel counts', () => {
    expect(formatPixels(0)).toBe('—');
    expect(formatPixels(640e6)).toBe('640 Mpx');
    expect(formatPixels(4.2e9)).toBe('4.2 Gpx');
    expect(formatPixels(2.5e10)).toBe('25 Gpx');
  });
});

describe('nightKeysFor', () => {
  it('spans only the nights the selected rigs and filters captured on', () => {
    const late = coverage('b', 'Rig B');
    late.nights = [{ night: '2026-10-01', target_id: 1, filter: 'L', frames: 1, accepted_frames: 0, seconds: 300, accepted_seconds: 0 }];
    const merged = mergeCoverage([coverage('a', 'Rig A'), late]);
    expect(nightKeysFor(merged, EVERYTHING)).toEqual(['2026-09-08', '2026-09-09', '2026-10-01']);
    expect(nightKeysFor(merged, { ...EVERYTHING, rigs: new Set(['b']) })).toEqual(['2026-10-01']);
    expect(nightKeysFor(merged, { ...EVERYTHING, filters: new Set(['Ha']) })).toEqual(['2026-09-08']);
    // A night with nothing accepted drops out of the span when only accepted frames count.
    expect(nightKeysFor(merged, { ...EVERYTHING, acceptedOnly: true })).toEqual(['2026-09-08', '2026-09-09']);
    // The time cuts do not shorten the span they run over.
    expect(nightKeysFor(merged, { ...EVERYTHING, fromNight: '2026-09-09', asOfNight: '2026-09-09' })).toHaveLength(3);
  });
});
