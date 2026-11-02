import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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
    // The red edges in words, for those who cannot see them.
    expect(strip).toHaveAccessibleDescription(/^Cloud cover: the lighter an hour, the more cloud\. Above the site's 30% threshold, with a red edge: .+ – .+\.$/);
  });

  it('draws no strip without a forecast', () => {
    const base = night(0);
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} />);
    expect(screen.queryByRole('group', { name: 'Cloud cover' })).toBeNull();
  });

  it('draws the night\'s overrides above the schedule, and moves a block with the arrow keys while editing', () => {
    const base = night(0);
    const block = {
      target: { project_id: 7, project_name: 'Heart Nebula', target_id: 70, target_name: 'IC 1805' },
      start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR, slew: true,
      runs: [{ exposure_plan_id: 700, filter: 'Ha', exposure_seconds: 300, frames: 24, dithers: 0, start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR }],
      decision: { kind: 'ranked' as const, candidates: [] },
    };
    const scheduled = { date: base.date, start_ms: base.noon_ms, end_ms: base.noon_ms + 24 * HOUR, blocks: [block],
      gaps: [{ start_ms: base.dusk_ms! + 5 * HOUR, end_ms: base.dawn_ms!, reason: 'held' as const, waiting_for: null }], frames: 24, exposure_hours: 2, note: null };
    const bands = [
      { kind: 'pin' as const, start_ms: block.start_ms, end_ms: block.end_ms, label: 'Only IC 1805 from 20:00 to 22:00' },
      { kind: 'hold' as const, start_ms: base.dusk_ms! + 5 * HOUR, end_ms: scheduled.end_ms, label: 'Hold the rig from 00:00' },
    ];
    const moves = vi.fn();
    const { rerender } = render(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} bands={bands} />);
    const row = screen.getByRole('group', { name: 'Overrides' });
    expect(within(row).getByTitle('Only IC 1805 from 20:00 to 22:00')).toHaveTextContent('Pinned');
    expect(within(row).getByTitle('Hold the rig from 00:00')).toHaveTextContent('Held');
    expect(screen.getByRole('list', { name: 'Legend' })).toHaveTextContent("Held: The night's overrides hold the rig.");
    // Without an edit open, a block does not move.
    const shown = screen.getByRole('button', { name: /IC 1805/ });
    expect(shown).not.toHaveAccessibleName(/arrow keys/);
    fireEvent.keyDown(shown, { key: 'ArrowRight' });
    rerender(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} bands={bands} onMoveBlock={moves} />);
    const movable = screen.getByRole('button', { name: /IC 1805/ });
    expect(movable).toHaveAccessibleName(/press the left and right arrow keys, to move it 15 minutes at a time/);
    fireEvent.keyDown(movable, { key: 'ArrowRight' });
    fireEvent.keyDown(movable, { key: 'ArrowLeft' });
    expect(moves.mock.calls.map(([moved, delta]) => [moved.start_ms, delta])).toEqual([[block.start_ms, 15 * 60_000], [block.start_ms, -15 * 60_000]]);
    // A click still chooses the block.
    fireEvent.click(movable);
    expect(movable).toHaveAttribute('aria-pressed', 'true');
  });
});
