import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import NightTimeline from '../NightTimeline';
import { night } from './fixtures';

const HOUR = 3_600_000;

describe('NightTimeline', () => {
  it('draws the forecast\'s cloud under the schedule, edging hours over the threshold', () => {
    const base = night(0);
    const clouds = [
      { start_ms: base.noon_ms + 7 * HOUR, end_ms: base.noon_ms + 8 * HOUR, cloud_total: 10, cloud_low: null, cloud_mid: null, cloud_high: null },
      { start_ms: base.noon_ms + 8 * HOUR, end_ms: base.noon_ms + 9 * HOUR, cloud_total: 75, cloud_low: 60, cloud_mid: null, cloud_high: 15 },
    ];
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={{ ...base, clouds }} scheduled={scheduled} colours={new Map()} cloudThreshold={30} />);
    const strip = screen.getByRole('group', { name: 'Cloud cover' });
    const hours = strip.querySelectorAll('.nights-cloud');
    expect(hours).toHaveLength(2);
    expect(hours[0]).not.toHaveClass('is-cloudy');
    expect(hours[1]).toHaveClass('is-cloudy');
    expect(hours[1].getAttribute('title')).toMatch(/: 75% cloud \(low 60%, high 15%\)$/);
    expect(within(screen.getByRole('group', { name: 'Schedule' })).queryByRole('group', { name: 'Cloud cover' })).toBe(strip);
  });

  it('draws no strip without a forecast', () => {
    const base = night(0);
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} />);
    expect(screen.queryByRole('group', { name: 'Cloud cover' })).toBeNull();
  });
});
