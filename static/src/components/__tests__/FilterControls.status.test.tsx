import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import FilterControls from '../FilterControls';

const mount = (status: string) => {
  const onFilterChange = vi.fn();
  render(
    <FilterControls
      onFilterChange={onFilterChange}
      availableFilters={['Ha']}
      currentFilters={{ status, filterName: 'all', dateRange: { start: null, end: null }, searchTerm: '' }}
    />
  );
  return onFilterChange;
};

describe('Status filter boxes', () => {
  it('ticks every grade for All, and unticking Rejected keeps the rest', () => {
    const onFilterChange = mount('all');
    const group = screen.getByRole('group', { name: 'Status:' });
    for (const name of ['Accepted', 'Rejected', 'Pending']) {
      expect(screen.getByRole('checkbox', { name })).toBeChecked();
    }
    expect(group).toBeInTheDocument();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Rejected' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'accepted,pending' }));
  });

  it('goes back to All when the last box is unticked', () => {
    const onFilterChange = mount('accepted');
    expect(screen.getByRole('checkbox', { name: 'Accepted' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Pending' })).not.toBeChecked();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Pending' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'accepted,pending' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Accepted' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'all' }));
  });
});
