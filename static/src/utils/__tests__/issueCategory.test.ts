import { describe, expect, it } from 'vitest';
import { formatCategory } from '../issueCategory';

describe('formatCategory', () => {
  it('names the fixed labels', () => {
    expect(formatCategory('satellite_trail_risk')).toBe('Satellite Trail Detected');
    expect(formatCategory('hfr_above_limit')).toBe('HFR Above Limit');
  });

  it('title-cases every other category, including sensor temperature', () => {
    expect(formatCategory('sensor_temperature')).toBe('Sensor Temperature');
    expect(formatCategory('likely_clouds')).toBe('Likely Clouds');
    expect(formatCategory('star_count_below_limit')).toBe('Star Count Below Limit');
  });
});
