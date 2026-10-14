import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import FilterControls from '../FilterControls';

const mount = (status: string) => {
  const onFilterChange = vi.fn();
  render(
    <FilterControls
      onFilterChange={onFilterChange}
      availableFilters={['Ha']}
      currentFilters={{ status, filterNames: [], dateRange: { start: null, end: null }, searchTerm: '' }}
    />
  );
  return onFilterChange;
};

describe('Status filter', () => {
  it('ticks every grade for All, and unticking Rejected keeps the rest', () => {
    const onFilterChange = mount('all');
    fireEvent.click(screen.getByRole('button', { name: 'Status: All' }));
    expect(screen.getByRole('group', { name: 'Status' })).toBeInTheDocument();
    for (const name of ['Accepted', 'Rejected', 'Pending']) {
      expect(screen.getByRole('checkbox', { name })).toBeChecked();
    }
    fireEvent.click(screen.getByRole('checkbox', { name: 'Rejected' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'accepted,pending' }));
  });

  it('goes back to All when the last box is unticked', () => {
    const onFilterChange = mount('accepted');
    fireEvent.click(screen.getByRole('button', { name: 'Status: Accepted' }));
    expect(screen.getByRole('checkbox', { name: 'Accepted' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Pending' })).not.toBeChecked();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Pending' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'accepted,pending' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Accepted' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'all' }));
  });

  it('marks the button of a field that narrows the view', () => {
    mount('accepted,pending');
    expect(screen.getByRole('button', { name: 'Status: Accepted, Pending' })).toHaveClass('is-set');
    expect(screen.getByRole('button', { name: 'Filter: All' })).not.toHaveClass('is-set');
  });
});

describe('Dates filter', () => {
  it('keeps the range in one field and clears it there', () => {
    const onFilterChange = vi.fn();
    render(
      <FilterControls
        onFilterChange={onFilterChange}
        availableFilters={[]}
        currentFilters={{ status: 'all', filterNames: [], dateRange: { start: '2026-04-16', end: null }, searchTerm: '' }}
      />
    );
    const dates = screen.getByRole('button', { name: /^Dates: From / });
    expect(dates).toHaveClass('is-set');
    fireEvent.click(dates);
    fireEvent.click(screen.getByRole('button', { name: 'Any date' }));
    expect(onFilterChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ dateRange: { start: null, end: null } })
    );
  });
});
