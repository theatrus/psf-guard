import { fireEvent, render, screen } from '@testing-library/react';
import { Images } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';
import ViewTab from '../ViewTab';

describe('ViewTab', () => {
  it('shows the current view by name and the others by icon, named on hover', () => {
    const open = vi.fn();
    const { rerender } = render(<ViewTab label="Images" icon={Images} current={false} onClick={open} />);
    const tab = screen.getByRole('button', { name: 'Images' });
    expect(tab).not.toHaveAttribute('aria-current');
    // The name waits in a flyout the screen reader skips; the button
    // carries it as its label.
    expect(tab.querySelector('.view-tab-flyout')).toHaveAttribute('aria-hidden', 'true');
    expect(tab.querySelector('.view-tab-name')).toBeNull();
    fireEvent.click(tab);
    expect(open).toHaveBeenCalledTimes(1);

    rerender(<ViewTab label="Images" icon={Images} current onClick={open} />);
    expect(tab).toHaveAttribute('aria-current', 'page');
    expect(tab.querySelector('.view-tab-name')).toHaveTextContent('Images');
    expect(tab.querySelector('.view-tab-flyout')).toBeNull();
  });

  it('says why a tab is off', () => {
    render(<ViewTab label="Images" icon={Images} current={false} disabled title="Choose a project or database first" onClick={vi.fn()} />);
    const tab = screen.getByRole('button', { name: 'Images' });
    expect(tab).toBeDisabled();
    expect(tab).toHaveAttribute('title', 'Choose a project or database first');
  });
});
