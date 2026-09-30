import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DatesPill, ProgressPill, StatePill } from '../projectPills';

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
});
