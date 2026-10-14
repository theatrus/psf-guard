import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CalibrationPill, DatesPill, ProgressPill, RowCells, StatePill } from '../projectPills';
import { ROW_COLUMNS } from '../projectRowColumns';

describe('project pills', () => {
  it('reads Done once the goal is met, a percent before, and a count without a goal', () => {
    const { rerender } = render(<ProgressPill accepted={40} desired={40} totalImages={44} />);
    expect(screen.getByText(/40 \/ 40 · Done/)).toHaveClass('is-done');
    rerender(<ProgressPill accepted={10} desired={40} totalImages={12} />);
    expect(screen.getByText(/10 \/ 40 · 25%/)).not.toHaveClass('is-done');
    rerender(<ProgressPill accepted={3} desired={0} totalImages={12} />);
    expect(screen.getByText('12 images')).toHaveClass('is-open');
  });

  it('names Target Scheduler states and skips a missing one', () => {
    const { container, rerender } = render(<StatePill state={2} />);
    expect(screen.getByText('Inactive')).toHaveClass('is-state-2');
    rerender(<StatePill state={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the capture span with how long ago the last frame was', () => {
    const now = Date.UTC(2026, 8, 29, 12);
    render(<DatesPill earliest={now / 1000 - 86_400 * 10} latest={now / 1000 - 86_400 * 3} nowMs={now} />);
    expect(screen.getByText(/3 d ago/)).toBeInTheDocument();
    render(<DatesPill earliest={null} latest={null} nowMs={now} />);
    expect(screen.getByText('No dates')).toBeInTheDocument();
  });

  it('writes every range at one width, so the dates line up down the Library', () => {
    const day = (month: number, date: number) => Date.UTC(2026, month, date, 12) / 1000;
    const now = Date.UTC(2026, 11, 1);
    const { container } = render(<>
      <DatesPill earliest={day(3, 6)} latest={day(3, 9)} nowMs={now} />
      <DatesPill earliest={day(9, 12)} latest={day(10, 30)} nowMs={now} />
    </>);
    const ranges = [...container.querySelectorAll('.library-dates-range')].map(range => range.textContent ?? '');
    expect(ranges).toHaveLength(2);
    expect(ranges[0].length).toBe(ranges[1].length);
    // The age is its own element, so a narrow Library can show it alone.
    expect(container.querySelectorAll('.library-dates-ago')).toHaveLength(2);
  });

  it('renders one cell per Library column, in order, empty where nothing is given', () => {
    const { container } = render(<div><RowCells cells={{ name: <span>Heart</span>, views: <button type="button">Sequence</button>, plan: false }} /></div>);
    const cells = [...container.firstElementChild!.children];
    expect(cells.map(cell => cell.getAttribute('data-column'))).toEqual([...ROW_COLUMNS]);
    expect(cells.map(cell => cell.textContent)).toEqual(['', 'Heart', '', '', '', '', '', '', 'Sequence', '']);
    expect(cells[0]).toHaveClass('library-cell', 'library-cell-fold');
  });

  it('shows calibration as an icon and one word, and opens the report', () => {
    const open = vi.fn();
    const health = { project_id: 3, level: 'poor' as const, poor_lights: 12, check_lights: 1, checked: 40, computed_at: 1 };
    const { container, rerender } = render(<CalibrationPill health={health} name="Bubble" onOpen={open} />);
    const pill = screen.getByRole('button', { name: 'Calibration Poor for Bubble' });
    expect(pill).toHaveTextContent('Poor');
    expect(pill).toHaveClass('is-poor');
    expect(pill).toHaveAttribute('title', "Calibration: 12 lights can't be calibrated properly; 1 light to check");
    fireEvent.click(pill);
    expect(open).toHaveBeenCalledTimes(1);

    rerender(<CalibrationPill health={{ ...health, level: 'check', poor_lights: 0, check_lights: 5 }} name="Bubble" onOpen={open} />);
    expect(screen.getByRole('button', { name: 'Calibration Check for Bubble' })).toHaveClass('is-check');
    // Nothing found, or no current result: no pill at all.
    rerender(<CalibrationPill health={{ ...health, level: null }} name="Bubble" onOpen={open} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<CalibrationPill health={undefined} name="Bubble" onOpen={open} />);
    expect(container).toBeEmptyDOMElement();
  });
});
