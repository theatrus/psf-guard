import { describe, expect, it } from 'vitest';
import type { CalibrationRuleField } from '../../api/types';
import { CALIBRATION_RULE_FIELDS } from '../../test/calibrationRuleFields';
import {
  RIG_PRESETS,
  checkOptions,
  describeRuleValue,
  draftOf,
  parseDraft,
  sameRuleValue,
} from '../calibrationRules';

const field = (key: string): CalibrationRuleField => {
  const found = CALIBRATION_RULE_FIELDS.find((entry) => entry.key === key);
  if (!found) throw new Error(`no field ${key}`);
  return found;
};

describe('calibration rules', () => {
  it('describes values the way the light\'s account does', () => {
    expect(describeRuleValue(field('rotation_tolerance_deg'), 2)).toBe('2°');
    expect(describeRuleValue(field('dark_temperature_tolerance_c'), 1.5)).toBe('1.5 °C');
    expect(describeRuleValue(field('flat_max_age_days'), 0)).toBe('no limit');
    expect(describeRuleValue(field('flat_check_age_days'), 0)).toBe('off');
    expect(describeRuleValue(field('thin_master_frames'), 4)).toBe('4 frames');
    expect(describeRuleValue(field('use_darks'), false)).toBe('off');
    expect(describeRuleValue(field('dark_choice'), 'nearest')).toBe('Nearest set');
    expect(describeRuleValue(field('check_no_flat'), 'advisory')).toBe('check only');
    expect(describeRuleValue(field('grade_flat_illumination'), { suspect: 0.05, poor: null }))
      .toBe('suspect 5%, poor never');
    expect(describeRuleValue(field('grade_dark_hot_pixels'), { suspect: 2, poor: 4 }))
      .toBe('suspect 2×, poor 4×');
  });

  it('round-trips a value through what its field shows', () => {
    for (const entry of CALIBRATION_RULE_FIELDS) {
      const parsed = parseDraft(entry, draftOf(entry, entry.default));
      expect(parsed.ok, entry.key).toBe(true);
      if (parsed.ok) expect(sameRuleValue(parsed.value!, entry.default), entry.key).toBe(true);
    }
  });

  it('reads an empty number as unset and checks ranges', () => {
    const reach = field('dark_reach_days');
    expect(parseDraft(reach, '')).toEqual({ ok: true, value: null });
    expect(parseDraft(reach, '0')).toEqual({ ok: false, error: 'Enter 1 to 3650 days.' });
    expect(parseDraft(field('rotation_tolerance_deg'), '181'))
      .toEqual({ ok: false, error: 'Enter 0 to 180 degrees.' });
    expect(parseDraft(field('thin_master_frames'), '3.5'))
      .toEqual({ ok: false, error: 'Enter 2 to 64 frames.' });
    // Wider than Seiza's default 3 °C: stacks check with the rig's own.
    expect(parseDraft(field('dark_temperature_tolerance_c'), '4').ok).toBe(true);
    expect(parseDraft(field('dark_temperature_tolerance_c'), '21').ok).toBe(false);
  });

  it('shows grade limits in percent where the field asks and orders them', () => {
    const dim = field('grade_flat_dim');
    expect(draftOf(dim, dim.default)).toEqual({ suspect: '10', poor: '3' });
    expect(parseDraft(dim, { suspect: '5', poor: '2' })).toEqual({ ok: true, value: { suspect: 0.05, poor: 0.02 } });
    // Lower is worse: suspect comes at the higher level.
    expect(parseDraft(dim, { suspect: '2', poor: '5' }).ok).toBe(false);
    expect(parseDraft(dim, { suspect: '', poor: '' })).toEqual({ ok: true, value: { suspect: null, poor: null } });
    expect(parseDraft(field('grade_flat_spots'), { suspect: '-1', poor: '' }).ok).toBe(false);
  });

  it('offers no advisory step for a check that only ever says check', () => {
    expect(checkOptions(field('check_no_flat')).map((option) => option.value)).toEqual(['flag', 'advisory', 'off']);
    expect(checkOptions(field('check_thin_bias')).map((option) => option.label)).toEqual(['Flag', 'Check only', 'Off']);
    expect(checkOptions(field('check_old_flats')).map((option) => option.value)).toEqual(['flag', 'off']);
  });

  it('keeps every preset to values its fields accept', () => {
    for (const preset of RIG_PRESETS) {
      for (const [key, value] of Object.entries(preset.rules)) {
        const entry = field(key);
        expect(parseDraft(entry, draftOf(entry, value)), `${preset.id}: ${key}`).toEqual({ ok: true, value });
      }
    }
  });
});
