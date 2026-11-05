import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import NightTimeline from '../NightTimeline';
import { OverrideChips } from '../TimelineEditing';
import type { NightOverride } from '../../../api/nightPlanTypes';
import { moonAltitudes, night } from './fixtures';

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
    expect(strip).toHaveAccessibleDescription(/^Cloud over 30%: .+ – .+$/);
  });

  it('draws the forecast\'s dew under the cloud, edging hours inside the dew limit', () => {
    const base = night(0);
    const air = (hour: number, temperature: number, dew: number | null) => ({
      start_ms: base.noon_ms + hour * HOUR, end_ms: base.noon_ms + (hour + 1) * HOUR, cloud_total: 0, cloud_low: null, cloud_mid: null, cloud_high: null,
      temperature_celsius: temperature, dew_point_celsius: dew, humidity_percent: 80,
    });
    // 8 °C above the dew point, then 1.4, then no dew point at all.
    const clouds = [air(7, 12, 4), air(8, 7, 5.6), air(9, 6, null)];
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={{ ...base, clouds }} scheduled={scheduled} colours={new Map()} cloudThreshold={30} dewLimit={2} />);
    const band = screen.getByRole('group', { name: 'Dew' });
    const hours = band.querySelectorAll('.nights-dew-hour');
    expect(hours).toHaveLength(2);
    expect(hours[0]).not.toHaveClass('is-dewy');
    expect(hours[1]).toHaveClass('is-dewy');
    expect(hours[1].getAttribute('title')).toMatch(/: 7 °C, dew point 6 °C, 80% humidity$/);
    // Nearer the dew point, stronger.
    const alpha = (hour: Element) => Number((hour as HTMLElement).style.getPropertyValue('--dew-alpha'));
    expect(alpha(hours[1])).toBeGreaterThan(alpha(hours[0]));
    expect(screen.getByRole('group', { name: 'Cloud cover' }).compareDocumentPosition(band) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(band).toHaveAccessibleDescription(/^Dew within 2 °C: .+ – .+$/);
  });

  it('draws no dew band for a forecast without the air', () => {
    const base = night(0);
    const clouds = [{ start_ms: base.noon_ms + 8 * HOUR, end_ms: base.noon_ms + 9 * HOUR, cloud_total: 20, cloud_low: null, cloud_mid: null, cloud_high: null }];
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={{ ...base, clouds }} scheduled={scheduled} colours={new Map()} dewLimit={2} />);
    expect(screen.getByRole('group', { name: 'Cloud cover' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Dew' })).toBeNull();
  });

  it('draws no strip without a forecast', () => {
    const base = night(0);
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} />);
    expect(screen.queryByRole('group', { name: 'Cloud cover' })).toBeNull();
  });

  const block = (base: ReturnType<typeof night>) => ({
    target: { project_id: 7, project_name: 'Heart Nebula', target_id: 70, target_name: 'IC 1805' },
    start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR, slew: true,
    runs: [{ exposure_plan_id: 700, filter: 'Ha', exposure_seconds: 300, frames: 24, dithers: 0, start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR }],
    decision: { kind: 'ranked' as const, candidates: [] },
  });
  const goals = [{ goal_id: 'e1', plan_id: 'p1', plan_name: 'Heart Nebula', target_guid: 't1', exposureplan_guid: 'e1', project_id: 7, target_id: 70, exposure_plan_id: 700, target_name: 'IC 1805', filter: 'Ha' }];
  const editing = (base: ReturnType<typeof night>, entries: NightOverride[] = []) => ({
    goals, entries, night: { start_ms: base.noon_ms, end_ms: base.noon_ms + 24 * HOUR }, dawn: base.dawn_ms!,
    onChange: vi.fn(), onMoveBlock: vi.fn(),
  });
  const scheduledFor = (base: ReturnType<typeof night>) => ({ date: base.date, start_ms: base.noon_ms, end_ms: base.noon_ms + 24 * HOUR, blocks: [block(base)], gaps: [], frames: 24, exposure_hours: 2, note: null });

  it('draws the saved overrides above the schedule, and leaves the blocks alone outside an edit', () => {
    const base = night(0);
    const scheduled = { ...scheduledFor(base), gaps: [{ start_ms: base.dusk_ms! + 5 * HOUR, end_ms: base.dawn_ms!, reason: 'held' as const, waiting_for: null }] };
    const bands = [
      { kind: 'pin' as const, start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR, label: 'Only IC 1805 from 20:00 to 22:00' },
      { kind: 'hold' as const, start_ms: base.dusk_ms! + 5 * HOUR, end_ms: scheduled.end_ms, label: 'Hold the rig from 00:00' },
    ];
    render(<NightTimeline night={base} scheduled={scheduled} colours={new Map()} bands={bands} />);
    const row = screen.getByRole('group', { name: 'Overrides' });
    expect(within(row).getByTitle('Only IC 1805 from 20:00 to 22:00')).toHaveTextContent('Pinned');
    expect(within(row).getByTitle('Hold the rig from 00:00')).toHaveTextContent('Held');
    expect(screen.getByRole('list', { name: 'Legend' })).toHaveTextContent('Held: An override stops the rig from starting work.');
    const shown = screen.getByRole('button', { name: /IC 1805/ });
    expect(shown).not.toHaveAccessibleName(/arrow keys/);
    expect(screen.queryByRole('slider', { name: 'Hold the rig from' })).toBeNull();
    // A click chooses the block, for why it was chosen.
    fireEvent.click(shown);
    expect(shown).toHaveAttribute('aria-pressed', 'true');
  });

  it('shows the closest the Moon comes to the target on each filter run, dimmed while the Moon stays down', () => {
    const base = night(0);
    const start = base.dusk_ms! + HOUR;
    const item = {
      ...block(base),
      runs: [
        { exposure_plan_id: 700, filter: 'Ha', exposure_seconds: 300, frames: 12, dithers: 0, start_ms: start, end_ms: start + HOUR, moon: { max_altitude_degrees: 31.4, min_separation_degrees: 42.2 } },
        { exposure_plan_id: 701, filter: 'OIII', exposure_seconds: 300, frames: 12, dithers: 0, start_ms: start + HOUR, end_ms: start + 2 * HOUR, moon: { max_altitude_degrees: -8, min_separation_degrees: 95 } },
        { exposure_plan_id: 700, filter: 'Ha', exposure_seconds: 300, frames: 12, dithers: 0, start_ms: start + 2 * HOUR, end_ms: start + 3 * HOUR, moon: { max_altitude_degrees: 20, min_separation_degrees: 38 } },
      ],
      end_ms: start + 3 * HOUR,
    };
    render(<NightTimeline night={base} scheduled={{ ...scheduledFor(base), blocks: [item] }} colours={new Map()} />);
    const up = screen.getByTitle(/ Ha: Moon 42° away, 31° up$/);
    // The block's own line stays first, since the runs cover the block.
    expect(up.getAttribute('title')).toMatch(/^IC 1805 · .+ – .+ · Ha ×24, OIII ×12\n/);
    expect(up).toHaveTextContent('Ha42°');
    expect(up.querySelector('.nights-run-moon')).not.toHaveAttribute('data-down');
    const down = screen.getByTitle(/ OIII: Moon 95° away, down$/);
    expect(down.querySelector('.nights-run-moon')).toHaveAttribute('data-down', 'true');
    expect(screen.queryByRole('list', { name: 'Legend' })).toBeNull();
    // Screen readers hear the Moon with the block.
    expect(screen.getByRole('button', { name: /IC 1805/ })).toHaveAccessibleName(/, 36 frames\. Ha: Moon 38° away, 31° up · OIII: Moon 95° away, down$/);
    fireEvent.click(screen.getByRole('button', { name: /IC 1805/ }));
    expect(within(screen.getByRole('region', { name: 'Why this target' })).getByText('Ha: Moon 38° away, 31° up · OIII: Moon 95° away, down')).toBeInTheDocument();
  });

  it('draws the night\'s Moon under the cloud band, from its rise to its set', () => {
    const base = night(0);
    const clouds = [{ start_ms: base.noon_ms + 8 * HOUR, end_ms: base.noon_ms + 9 * HOUR, cloud_total: 20, cloud_low: null, cloud_mid: null, cloud_high: null }];
    const lit = { ...base, moon_illumination: 0.42, moon_altitudes: moonAltitudes(base.noon_ms), clouds };
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={lit} scheduled={scheduled} colours={new Map()} />);
    const strip = screen.getByRole('img', { name: /^Moon: 42% lit · rises .+ · highest .+ · sets .+$/ });
    expect(strip.style.backgroundImage).toMatch(/^linear-gradient\(to right, /);
    // The same words under the axis, so the strip itself stays clear.
    expect(screen.getByText(/^42% lit · rises .+ · highest .+ · sets .+$/)).toHaveClass('nights-moon-caption');
    expect(screen.getByRole('group', { name: 'Cloud cover' }).compareDocumentPosition(strip) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('says when the Moon stays down all night', () => {
    const base = night(0);
    const down = { ...base, moon_illumination: 0.03, moon_altitudes: { ...moonAltitudes(base.noon_ms), degrees: Array(96).fill(-10) } };
    const scheduled = { date: base.date, start_ms: base.dusk_ms!, end_ms: base.dawn_ms!, blocks: [], gaps: [], frames: 0, exposure_hours: 0, note: null };
    render(<NightTimeline night={down} scheduled={scheduled} colours={new Map()} />);
    expect(screen.getByRole('img', { name: 'Moon: 3% lit · down all night' })).toBeInTheDocument();
  });

  it('opens a menu on a block while editing, whose actions change the overrides', () => {
    const base = night(0);
    const edit = editing(base);
    render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={edit} />);
    const shown = screen.getByRole('button', { name: /IC 1805/ });
    expect(shown).toHaveAccessibleName(/press the left and right arrow keys, to move it 15 minutes at a time/);
    fireEvent.keyDown(shown, { key: 'ArrowRight' });
    expect(edit.onMoveBlock.mock.calls.map(([moved, delta]) => [moved.start_ms, delta])).toEqual([[base.dusk_ms! + HOUR, 15 * 60_000]]);
    fireEvent.click(shown);
    const menu = screen.getByRole('menu', { name: /^IC 1805, .+, 24 frames$/ });
    expect(within(menu).getByRole('menuitem', { name: 'Pin it here' })).toHaveFocus();
    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Pin it here' }));
    expect(edit.onChange).toHaveBeenLastCalledWith([{ kind: 'pin', subject: { kind: 'target', target_guid: 't1' }, window: { start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR } }]);
    expect(screen.queryByRole('menu')).toBeNull();
    fireEvent.click(shown);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Leave it out tonight' }));
    expect(edit.onChange).toHaveBeenLastCalledWith([{ kind: 'exclude', subject: { kind: 'target', target_guid: 't1' } }]);
    fireEvent.click(shown);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Move it 30 minutes later' }));
    expect(edit.onMoveBlock).toHaveBeenLastCalledWith(expect.objectContaining({ start_ms: base.dusk_ms! + HOUR }), 30 * 60_000);
    // Escape closes the menu without a change.
    fireEvent.click(shown);
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    fireEvent.click(shown);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Why this target' }));
    expect(screen.getByRole('region', { name: 'Why this target' })).toBeInTheDocument();
  });

  it('moves a dragged block by the time dragged, and a drag opens no menu', () => {
    const base = night(0);
    // The drawn span, an hour before dusk to an hour after dawn, is 12 h:
    // 1200 px make 100 px an hour.
    const width = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ width: 1200, height: 38, x: 0, y: 0, top: 0, left: 0, right: 1200, bottom: 38, toJSON: () => ({}) } as DOMRect);
    const edit = editing(base);
    render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={edit} />);
    const shown = screen.getByRole('button', { name: /IC 1805/ });
    fireEvent.pointerDown(shown, { clientX: 300, pointerId: 1 });
    fireEvent.pointerMove(shown, { clientX: 350, pointerId: 1 });
    fireEvent.pointerUp(shown, { clientX: 350, pointerId: 1 });
    fireEvent.click(shown);
    expect(edit.onMoveBlock).toHaveBeenCalledTimes(1);
    expect(edit.onMoveBlock.mock.calls[0][1]).toBe(30 * 60_000);
    expect(screen.queryByRole('menu')).toBeNull();
    width.mockRestore();
  });

  it('moves and resizes a pin by its band, and removes it from its menu', () => {
    const base = night(0);
    const window = { start_ms: base.dusk_ms! + HOUR, end_ms: base.dusk_ms! + 3 * HOUR };
    const pin: NightOverride = { kind: 'pin', subject: { kind: 'target', target_guid: 't1' }, window };
    const edit = editing(base, [pin]);
    render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={edit} />);
    const row = screen.getByRole('group', { name: 'Overrides' });
    fireEvent.keyDown(within(row).getByRole('button', { name: /^Only IC 1805 from/ }), { key: 'ArrowRight' });
    expect(edit.onChange).toHaveBeenLastCalledWith([{ ...pin, window: { start_ms: window.start_ms + 15 * 60_000, end_ms: window.end_ms + 15 * 60_000 } }]);
    fireEvent.keyDown(within(row).getByRole('slider', { name: /^End of: Only IC 1805/ }), { key: 'ArrowLeft' });
    expect(edit.onChange).toHaveBeenLastCalledWith([{ ...pin, window: { start_ms: window.start_ms, end_ms: window.end_ms - 15 * 60_000 } }]);
    fireEvent.click(within(row).getByRole('button', { name: /^Only IC 1805 from/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Remove this pin' }));
    expect(edit.onChange).toHaveBeenLastCalledWith([]);
  });

  it('holds the rig from where the hold line is set, and lifts the hold at the end of the dark', () => {
    const base = night(0);
    const edit = editing(base);
    const { rerender } = render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={edit} />);
    const line = screen.getByRole('slider', { name: 'Hold the rig from' });
    expect(line).toHaveAttribute('aria-valuetext', 'No hold');
    fireEvent.keyDown(line, { key: 'ArrowLeft' });
    expect(edit.onChange).toHaveBeenLastCalledWith([{ kind: 'hold', from_ms: base.dawn_ms! - 15 * 60_000 }]);
    const held = editing(base, [{ kind: 'hold', from_ms: base.dawn_ms! - 15 * 60_000 }]);
    rerender(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={held} />);
    expect(screen.getByRole('slider', { name: 'Hold the rig from' })).toHaveAttribute('aria-valuetext', expect.stringMatching(/^Hold from /));
    fireEvent.keyDown(screen.getByRole('slider', { name: 'Hold the rig from' }), { key: 'ArrowRight' });
    expect(held.onChange).toHaveBeenLastCalledWith([]);
    fireEvent.keyDown(screen.getByRole('slider', { name: 'Hold the rig from' }), { key: 'Home' });
    expect(held.onChange).toHaveBeenLastCalledWith([{ kind: 'hold', from_ms: base.noon_ms }]);
  });

  it('stands a whole night\'s hold at the start of the drawn span, where a press changes nothing', () => {
    const base = night(0);
    const edit = editing(base, [{ kind: 'hold' }]);
    render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={edit} />);
    const line = screen.getByRole('slider', { name: 'Hold the rig from' });
    expect(line).toHaveAttribute('aria-valuetext', 'Hold all night');
    expect(line).toHaveStyle({ left: '0%' });
    fireEvent.pointerDown(line, { clientX: 0, pointerId: 1 });
    fireEvent.pointerUp(line, { clientX: 0, pointerId: 1 });
    // Already as early as it goes.
    fireEvent.keyDown(line, { key: 'ArrowLeft' });
    expect(edit.onChange).not.toHaveBeenCalled();
    // One step later than the drawn span's start, an hour before dusk.
    fireEvent.keyDown(line, { key: 'ArrowRight' });
    expect(edit.onChange).toHaveBeenLastCalledWith([{ kind: 'hold', from_ms: base.dusk_ms! - HOUR + 15 * 60_000 }]);
  });

  it('keeps a menu\'s focus where the keyboard moved it while the page draws again', () => {
    const base = night(0);
    const { rerender } = render(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={editing(base)} />);
    fireEvent.click(screen.getByRole('button', { name: /IC 1805/ }));
    const menu = screen.getByRole('menu');
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    const chosen = within(menu).getByRole('menuitem', { name: /^Leave it out from/ });
    expect(chosen).toHaveFocus();
    // The page's clock draws it again, with a new edit and new callbacks.
    rerender(<NightTimeline night={base} scheduled={scheduledFor(base)} colours={new Map()} edit={editing(base)} />);
    expect(chosen).toHaveFocus();
  });
});

describe('OverrideChips', () => {
  const goals = [
    { goal_id: 'e1', plan_id: 'p1', plan_name: 'Heart Nebula', target_guid: 't1', exposureplan_guid: 'e1', project_id: 7, target_id: 70, exposure_plan_id: 700, target_name: 'IC 1805', filter: 'Ha' },
    { goal_id: 'e2', plan_id: 'p2', plan_name: 'Soul Nebula', target_guid: 't2', exposureplan_guid: 'e2', project_id: 8, target_id: 80, exposure_plan_id: 800, target_name: 'IC 1848', filter: 'Ha' },
  ];

  it('lists what the timeline cannot draw, and sets the night\'s plan order while editing', () => {
    const left: NightOverride = { kind: 'exclude', subject: { kind: 'plan', plan_id: 'p2' } };
    const changes = vi.fn();
    const { rerender } = render(<OverrideChips entries={[left]} goals={goals} editing={false} onChange={changes} />);
    expect(screen.getByRole('group', { name: 'Overrides for the whole night' })).toHaveTextContent('Leave out Soul Nebula all night');
    expect(screen.queryByRole('button', { name: /Remove/ })).toBeNull();
    rerender(<OverrideChips entries={[left]} goals={goals} editing onChange={changes} />);
    fireEvent.click(screen.getByRole('button', { name: 'Set a plan order' }));
    const menu = screen.getByRole('menu', { name: "Tonight's plan order, first first" });
    fireEvent.click(within(menu).getByRole('button', { name: 'Move Soul Nebula up' }));
    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Use this order tonight' }));
    expect(changes).toHaveBeenLastCalledWith([left, { kind: 'order', plans: ['p2', 'p1'] }]);
    fireEvent.click(screen.getByRole('button', { name: 'Remove: Leave out Soul Nebula all night' }));
    expect(changes).toHaveBeenLastCalledWith([]);
  });
});
