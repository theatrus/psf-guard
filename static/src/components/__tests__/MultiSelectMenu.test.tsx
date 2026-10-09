import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import MultiSelectMenu from '../MultiSelectMenu';

const options = [{ value: 'L', label: 'L' }, { value: 'R', label: 'R' }, { value: 'G', label: 'G' }];

function mount(chosen: string[], everyIsAll = false) {
  const onChange = vi.fn();
  render(<><MultiSelectMenu id="menu" label="Filter:" options={options} chosen={chosen} onChange={onChange} everyIsAll={everyIsAll} /><p>outside</p></>);
  return onChange;
}

describe('MultiSelectMenu', () => {
  it('says what it keeps and keeps the list order whatever the ticking order', () => {
    const onChange = mount(['G']);
    const button = screen.getByRole('button', { name: 'Filter: G' });
    fireEvent.click(button);
    fireEvent.click(screen.getByRole('checkbox', { name: 'L' }));
    expect(onChange).toHaveBeenLastCalledWith(['L', 'G']);
  });

  it('clears with All, and only a filter list counts every box as All', () => {
    const flags = mount(['L', 'R']);
    fireEvent.click(screen.getByRole('button', { name: /^Filter:/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'G' }));
    expect(flags).toHaveBeenLastCalledWith(['L', 'R', 'G']);
    fireEvent.click(screen.getByRole('checkbox', { name: 'All' }));
    expect(flags).toHaveBeenLastCalledWith([]);
  });

  it('treats every filter ticked as All when asked to', () => {
    const onChange = mount(['L', 'R'], true);
    fireEvent.click(screen.getByRole('button', { name: /^Filter:/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'G' }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it('closes on Escape, back to its button, and on a click outside', () => {
    mount([]);
    const button = screen.getByRole('button', { name: 'Filter: All' });
    fireEvent.click(button);
    expect(screen.getByRole('group', { name: 'Filter' })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('group', { name: 'Filter' })).not.toBeInTheDocument();
    expect(button).toHaveFocus();
    fireEvent.click(button);
    fireEvent.pointerDown(screen.getByText('outside'));
    expect(screen.queryByRole('group', { name: 'Filter' })).not.toBeInTheDocument();
  });
});
