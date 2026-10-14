import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import ViewToolbar from '../ViewToolbar';
import SelectionBar from '../SelectionBar';

describe('ViewToolbar', () => {
  it('lays out its slots and folds the filters and view settings behind one Filters button', () => {
    const onClearFilters = vi.fn();
    render(
      <ViewToolbar
        label="Image tools"
        className="image-controls"
        sticky
        filters={<label>Filter<select><option>All</option></select></label>}
        activeFilters={2}
        onClearFilters={onClearFilters}
        view={<label>Size<input type="range" /></label>}
        tools={<button type="button">Compare</button>}
        summary={<span>3 of 9 images</span>}
        selection={<span>2 selected</span>}
      />
    );
    const toolbar = screen.getByRole('region', { name: 'Image tools' });
    expect(toolbar).toHaveClass('view-toolbar', 'image-controls', 'sticky', 'has-view');
    expect(within(toolbar).getByText('3 of 9 images')).toBeInTheDocument();
    expect(within(toolbar).getByRole('button', { name: 'Compare' })).toBeInTheDocument();

    // The narrow-screen button says how many filters are in use and opens
    // the panel that holds them; on a wide screen the CSS hides it.
    const toggle = within(toolbar).getByRole('button', { name: 'Filters (2 in use)' });
    const panel = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(panel).toHaveClass('view-toolbar-panel');
    expect(within(panel).getByLabelText('Filter')).toBeInTheDocument();
    expect(within(panel).getByLabelText('Size')).toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toolbar).not.toHaveClass('is-open');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(toolbar).toHaveClass('is-open');
    // The tools are not in the panel: they stay in sight when it folds.
    expect(within(panel).queryByRole('button', { name: 'Compare' })).toBeNull();

    fireEvent.click(within(panel).getByRole('button', { name: 'Clear filters' }));
    expect(onClearFilters).toHaveBeenCalledTimes(1);
  });

  it('offers Clear filters only once a filter is in use, and no Filters button without filters', () => {
    const { rerender } = render(
      <ViewToolbar label="Masters" filters={<span>Type</span>} activeFilters={0} onClearFilters={() => {}} />
    );
    expect(screen.getByRole('button', { name: 'Clear filters' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Filters' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Masters' })).not.toHaveClass('has-view');

    rerender(<ViewToolbar label="Masters" tools={<button type="button">Clear generated masters</button>} />);
    expect(screen.queryByRole('button', { name: /^Filters/ })).toBeNull();
  });
});

describe('SelectionBar', () => {
  it('rests with a count and a hint, and turns into the actions once enough are selected', () => {
    const onClear = vi.fn();
    const { rerender } = render(
      <SelectionBar count={1} minimum={2} hint="Shift-click for a range" onClear={onClear}>
        <button type="button">Accept</button>
      </SelectionBar>
    );
    const bar = screen.getByRole('group', { name: 'Selected image actions' });
    expect(bar).not.toHaveClass('active');
    expect(bar).toHaveTextContent('1 selected');
    expect(bar).toHaveTextContent('Shift-click for a range');
    expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull();

    rerender(
      <SelectionBar count={3} minimum={2} hint="Shift-click for a range" note="Rule: Off target" onClear={onClear}>
        <button type="button">Accept</button>
      </SelectionBar>
    );
    expect(bar).toHaveClass('active');
    expect(bar).toHaveTextContent('3 selected');
    expect(bar).toHaveTextContent('Rule: Off target');
    expect(screen.getByRole('button', { name: 'Accept' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onClear).toHaveBeenCalledTimes(1);
  });
});
