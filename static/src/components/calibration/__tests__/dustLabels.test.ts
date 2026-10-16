import { describe, expect, it } from 'vitest';
import { featureLabel, featureMark, isLarge, isRound, spotWords, spotsSummary, whereDust } from '../dustLabels';

describe('dust labels', () => {
  it('reads spots and large dust as one', () => {
    expect(spotsSummary(15, 3, ['disk', 'disk', 'disk'])).toBe('15 spots · 3 large disks');
    expect(spotsSummary(1, 1, ['donut'])).toBe('1 spot · 1 donut');
    expect(spotsSummary(0, 0)).toBe('0 spots');
    expect(spotsSummary(4.5, 2.5, ['disk'])).toBe('4.5 spots · 2.5 large disks');
    expect(spotsSummary(null, 2)).toBe('—');
  });

  it('says where dust that far from the sensor sits', () => {
    expect(whereDust(1.2)).toBe('sensor glass');
    expect(whereDust(6.5)).toBe('camera window');
    expect(whereDust(10.9)).toBe('filter/window');
    expect(whereDust(49)).toBe('further up the train');
  });

  it('names a feature by shape, depth and distance', () => {
    const disk = { x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, pixels: 1, depth: 0.0033, shape: 'disk' as const, distance_mm: 10.9 };
    expect(featureLabel(disk)).toBe('disk 0.33% · ~11 mm, filter/window');
    expect(featureLabel({ ...disk, shape: undefined, distance_mm: undefined, depth: 0.12 })).toBe('spot 12.00%');
  });

  it('says what became of a spot between two sessions', () => {
    const spot = { kind: 'new' as const, darker: true, x: 0, y: 0, radius: 300, peak_pct: 0.3 };
    expect(spotWords(spot)).toBe('New');
    expect(spotWords({ ...spot, shape: 'disk' })).toBe('New disk');
    expect(spotWords({ ...spot, kind: 'moved', shape: 'donut', within_tolerance: true })).toBe('Moved donut within focus travel');
    expect(spotWords({ ...spot, kind: 'gone', shape: 'patch' })).toBe('Gone patch of disks');
  });

  it('draws a patch of disks by its box, as large dust', () => {
    const patch = { x: 500, y: 400, left: 300, top: 250, right: 700, bottom: 520, pixels: 90_000, depth: 0.002, shape: 'patch' as const, radius: 169 };
    expect(isLarge('patch')).toBe(true);
    expect(isRound('patch')).toBe(false);
    const mark = featureMark(patch, 'p');
    expect(mark.tone).toBe('disk');
    expect(mark.bounds).toEqual([300, 250, 700, 520]);
    expect(mark.label).toBe('patch of disks 0.20%');
  });
});
