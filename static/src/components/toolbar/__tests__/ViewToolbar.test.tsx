import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import ViewToolbar from '../ViewToolbar';
import SelectionBar from '../SelectionBar';
import PillSelect from '../PillSelect';

function renderToolbar(activeFilters = 2) {
  const onClearFilters = vi.fn();
  const onMove = vi.fn();
  render(
    <ViewToolbar
      label="Image tools"
      className="image-controls"
      sticky
      filters={(
        <PillSelect label="Type" active value="flat" onChange={() => {}}>
          <option value="">All</option>
          <option value="flat">Flat</option>
        </PillSelect>
      )}
      activeFilters={activeFilters}
      onClearFilters={onClearFilters}
      controls={<label>Size<input type="range" /></label>}
      primary={<button type="button">Rule</button>}
      view={<label>Group<select><option>Filter</option></select></label>}
      tools={<button type="button">Compare</button>}
      menu={<button type="button" onClick={onMove}>Move exposures</button>}
      summary={<span>3 of 9 images</span>}
      selection={<span>2 selected</span>}
    />
  );
  return { onClearFilters, onMove, toolbar: screen.getByRole('region', { name: 'Image tools' }) };
}

describe('ViewToolbar', () => {
  it('keeps filters, sliders, the selection rule and the tools in one control row', () => {
    const { toolbar } = renderToolbar();
    expect(toolbar).toHaveClass('view-toolbar', 'image-controls', 'sticky');
    const bar = toolbar.querySelector('.view-toolbar-bar')!;
    expect(within(bar as HTMLElement).getByLabelText('Type')).toHaveValue('flat');
    expect(within(bar as HTMLElement).getByLabelText('Size')).toBeInTheDocument();
    expect(within(bar as HTMLElement).getByRole('button', { name: 'Rule' })).toBeInTheDocument();
    expect(within(bar as HTMLElement).getByRole('button', { name: 'Compare' })).toBeInTheDocument();
    expect(within(toolbar.querySelector('.view-toolbar-status') as HTMLElement).getByText('3 of 9 images')).toBeInTheDocument();
    // A pill that narrows the view stands out.
    expect(screen.getByLabelText('Type').closest('.toolbar-pill')).toHaveClass('is-set');
  });

  it('folds the filters and sliders behind one Filters button that counts the filters', () => {
    const { toolbar } = renderToolbar();
    // On a wide screen the CSS hides this button and shows the row as is.
    const toggle = within(toolbar).getByRole('button', { name: 'Filters (2 in use)' });
    const panel = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(panel).toHaveClass('view-toolbar-filters');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toolbar).not.toHaveClass('is-open');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(toolbar).toHaveClass('is-open');
    // The tools and the selection rule are not in the panel.
    expect(within(panel).queryByRole('button', { name: 'Compare' })).toBeNull();
    expect(within(panel).queryByRole('button', { name: 'Rule' })).toBeNull();
  });

  it('opens View and ⋯ as panels, and ⋯ closes once an action is used', () => {
    const { onMove } = renderToolbar();
    const view = screen.getByRole('button', { name: 'View' });
    expect(screen.queryByRole('group', { name: 'View' })).toBeNull();
    fireEvent.click(view);
    expect(view).toHaveAttribute('aria-expanded', 'true');
    expect(within(screen.getByRole('group', { name: 'View' })).getByLabelText('Group')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(view).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(screen.getByRole('button', { name: 'More tools' }));
    fireEvent.click(within(screen.getByRole('group', { name: 'More tools' })).getByRole('button', { name: 'Move exposures' }));
    expect(onMove).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'More tools' })).toHaveAttribute('aria-expanded', 'false');
  });

  it('offers Clear only once a filter is in use', () => {
    const { onClearFilters } = renderToolbar();
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(onClearFilters).toHaveBeenCalledTimes(1);
  });

  it('has no Clear without a filter in use, and no Filters button with nothing to fold', () => {
    const { rerender } = render(
      <ViewToolbar label="Masters" filters={<span>Type</span>} activeFilters={0} onClearFilters={() => {}} />
    );
    expect(screen.queryByRole('button', { name: 'Clear filters' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Filters' })).toBeInTheDocument();

    rerender(<ViewToolbar label="Masters" menu={<button type="button">Clear generated masters</button>} />);
    expect(screen.queryByRole('button', { name: /^Filters/ })).toBeNull();
    expect(screen.getByRole('button', { name: 'More tools' })).toBeInTheDocument();
  });
});

describe('SelectionBar', () => {
  it('rests as a count whose tooltip says how to select, and turns into the actions once enough are selected', () => {
    const onClear = vi.fn();
    const { rerender } = render(
      <SelectionBar count={1} minimum={2} hint="Shift-click for a range" onClear={onClear}>
        <button type="button">Accept</button>
      </SelectionBar>
    );
    const bar = screen.getByRole('group', { name: 'Selected image actions' });
    expect(bar).not.toHaveClass('active');
    expect(bar).toHaveTextContent('1 selected');
    expect(screen.getByText('1 selected')).toHaveAttribute('title', 'Shift-click for a range');
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
