import { fireEvent, render, screen } from '@testing-library/react';
import { Images } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';
import ViewTab from '../ViewTab';

describe('ViewTab', () => {
  it('carries its name as its label, and marks the current view', () => {
    const open = vi.fn();
    const { rerender } = render(<ViewTab label="Images" icon={Images} current={false} onClick={open} />);
    const tab = screen.getByRole('button', { name: 'Images' });
    expect(tab).not.toHaveAttribute('aria-current');
    // The name slides out of the button on hover; the styles decide when
    // it shows, so it is always there, hidden from the screen reader,
    // which reads the label.
    expect(tab.querySelector('.view-tab-name')).toHaveTextContent('Images');
    expect(tab.querySelector('.view-tab-name')).toHaveAttribute('aria-hidden', 'true');
    fireEvent.click(tab);
    expect(open).toHaveBeenCalledTimes(1);

    rerender(<ViewTab label="Images" icon={Images} current onClick={open} />);
    expect(tab).toHaveAttribute('aria-current', 'page');
  });

  it('says why a tab is off', () => {
    render(<ViewTab label="Images" icon={Images} current={false} disabled title="Choose a project or database first" onClick={vi.fn()} />);
    const tab = screen.getByRole('button', { name: 'Images' });
    expect(tab).toBeDisabled();
    expect(tab).toHaveAttribute('title', 'Choose a project or database first');
  });
});
