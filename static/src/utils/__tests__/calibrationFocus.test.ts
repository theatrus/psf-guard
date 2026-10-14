import { describe, expect, it } from 'vitest';
import type { CalibrationKindExplanation, LightCalibrationFinding } from '../../api/types';
import {
  NO_FINDING,
  chipFocus,
  chosenFinding,
  focusParts,
  nightsApart,
  temperatureRange,
} from '../calibrationFocus';

const findings: LightCalibrationFinding[] = [
  { code: 'thin_dark', level: 'poor', reason: 'Its dark master would use only 3 frames', kinds: ['dark'] },
  { code: 'mixed_flats', level: 'check', reason: 'Its flat master mixes flats from 2 nights', kinds: ['flat'] },
];

function kind(name: CalibrationKindExplanation['kind'], frames = 10): CalibrationKindExplanation {
  return {
    kind: name,
    master: { frames, nights: frames > 0 ? 1 : 0 },
    used: [],
    matching_unused: [],
    refused: [],
    kept_out: [],
    suspect: [],
    other_filters: 0,
    other_cameras: 0,
  };
}

describe('the calibration panel focus', () => {
  it('takes the chosen finding, else the Images filter, else the worst', () => {
    expect(chosenFinding(findings, 'mixed_flats', [])?.code).toBe('mixed_flats');
    expect(chosenFinding(findings, null, ['mixed_flats'])?.code).toBe('mixed_flats');
    expect(chosenFinding(findings, null, ['check'])?.code).toBe('mixed_flats');
    expect(chosenFinding(findings, null, [])?.code).toBe('thin_dark');
    // A choice the light no longer has falls back the same way.
    expect(chosenFinding(findings, 'old_flats', ['check'])?.code).toBe('mixed_flats');
    expect(chosenFinding(findings, NO_FINDING, ['mixed_flats'])).toBeNull();
    expect(chosenFinding([], null, [])).toBeNull();
  });

  it('points a finding at its own kind only', () => {
    const [thinDark, mixedFlats] = findings;
    expect([...focusParts(thinDark, kind('dark'))]).toEqual(['master', 'used', 'kept_out']);
    expect(focusParts(thinDark, kind('flat')).size).toBe(0);
    expect([...focusParts(mixedFlats, kind('flat'))]).toEqual(['used']);
    expect(focusParts(null, kind('flat')).size).toBe(0);
  });

  it('shows why nothing matched, or the nights of flats too old', () => {
    const cannot: LightCalibrationFinding = {
      code: 'cannot_calibrate', level: 'poor', reason: 'No matching flat', kinds: ['flat'],
    };
    expect([...focusParts(cannot, kind('flat', 0))]).toEqual(['refused', 'other']);
    expect([...focusParts(cannot, kind('flat', 5))]).toEqual(['used']);
  });

  it("opens a chip on the finding Images is filtered by, else the light's first", () => {
    expect(chipFocus(['thin_dark', 'mixed_flats'], ['mixed_flats'])).toBe('mixed_flats');
    expect(chipFocus(['thin_dark', 'mixed_flats'], ['poor'])).toBe('thin_dark');
    expect(chipFocus(['thin_dark', 'mixed_flats'], [])).toBe('thin_dark');
  });

  it('counts days between nights and writes temperature ranges', () => {
    expect(nightsApart('2026-10-06', '2026-10-01')).toBe(5);
    expect(nightsApart('2026-10-06', null)).toBeNull();
    expect(nightsApart('unknown', '2026-10-01')).toBeNull();
    expect(temperatureRange(-10, -10)).toBe('−10.0 °C');
    expect(temperatureRange(-10.2, -9.8)).toBe('−10.2 to −9.8 °C');
    expect(temperatureRange(undefined, -9.8)).toBeNull();
  });
});
