import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SkyTimeline from '../SkyTimeline';
import type { Lane } from '../skyModel';

const nightKeys = ['2025-11-02', '2025-12-14', '2026-01-09', '2026-02-20'];

const lanes: Lane[] = [{
  db_id: 'db-1',
  db_name: 'Rig one',
  seconds: 4 * 3600,
  perNight: new Map(nightKeys.map((night) => [night, { seconds: 3600, byFilter: new Map([['Ha', 3600]]) }])),
}];

function renderTimeline(fromNight: string | null, onRange = vi.fn()) {
  render(
    <SkyTimeline
      lanes={lanes}
      nightKeys={nightKeys}
      fromNight={fromNight}
      asOfNight={null}
      onRange={onRange}
      playing={false}
      onTogglePlay={() => undefined}
    />,
  );
  return onRange;
}

describe('SkyTimeline start year', () => {
  it('says the whole span is shown when no start year is chosen', () => {
    renderTimeline(null);
    expect(screen.getByRole('button', { name: 'From the start' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'From 2026' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('says which start year is chosen', async () => {
    const onRange = renderTimeline('2026-01-09');
    expect(screen.getByRole('button', { name: 'From 2026' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'From 2025' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: 'From the start' })).toHaveAttribute('aria-pressed', 'false');
    await userEvent.click(screen.getByRole('button', { name: 'From the start' }));
    expect(onRange).toHaveBeenCalledWith(null, null);
  });
});
