import { describe, expect, it } from 'vitest';
import { calibrationFilterLabel, matchesCalibrationFilter } from '../calibrationFilter';
import type { CalibrationLightMark } from '../../api/types';

const poor: CalibrationLightMark = { image_id: 1, level: 'poor', codes: ['cannot_calibrate'] };
const check: CalibrationLightMark = { image_id: 2, level: 'check', codes: ['old_flats'] };

describe('calibration filter', () => {
  it('keeps everything when nothing is chosen', () => {
    expect(matchesCalibrationFilter([], undefined)).toBe(true);
    expect(matchesCalibrationFilter([], poor)).toBe(true);
  });

  it('matches a light by its worst level or by any of its codes', () => {
    expect(matchesCalibrationFilter(['poor'], poor)).toBe(true);
    expect(matchesCalibrationFilter(['poor'], check)).toBe(false);
    expect(matchesCalibrationFilter(['old_flats'], check)).toBe(true);
    expect(matchesCalibrationFilter(['cannot_calibrate'], check)).toBe(false);
    expect(matchesCalibrationFilter(['poor'], undefined)).toBe(false);
  });

  it('says each value in a few words', () => {
    expect(calibrationFilterLabel('poor')).toBe('Poor');
    expect(calibrationFilterLabel('cannot_calibrate')).toBe("Can't be calibrated");
    expect(calibrationFilterLabel('something_new')).toBe('something_new');
  });
});
