import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import WbppOptionsFields from '../WbppOptionsFields';
import { DEFAULT_WBPP_OPTIONS } from '../../api/types';

describe('WbppOptionsFields', () => {
  it('reports each change on top of the current value, and clears the optional ones', () => {
    const onChange = vi.fn();
    render(<WbppOptionsFields value={DEFAULT_WBPP_OPTIONS} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText(/^Quality/), { target: { value: 'fast' } });
    expect(onChange).toHaveBeenLastCalledWith({ ...DEFAULT_WBPP_OPTIONS, quality: 'fast' });
    fireEvent.change(screen.getByLabelText(/^Fast Integration/), { target: { value: 'auto' } });
    expect(onChange).toHaveBeenLastCalledWith({
      ...DEFAULT_WBPP_OPTIONS,
      fast_integration: 'auto',
    });
    fireEvent.change(screen.getByLabelText(/^Light rejection/), {
      target: { value: 'winsorized_sigma' },
    });
    expect(onChange).toHaveBeenLastCalledWith({
      ...DEFAULT_WBPP_OPTIONS,
      rejection: 'winsorized_sigma',
    });
    fireEvent.change(screen.getByLabelText(/^Autocrop/), { target: { value: 'on' } });
    expect(onChange).toHaveBeenLastCalledWith({ ...DEFAULT_WBPP_OPTIONS, autocrop: true });
  });

  it('shows the optional settings as WBPP defaults when unset', () => {
    render(
      <WbppOptionsFields
        value={{ ...DEFAULT_WBPP_OPTIONS, autocrop: false, rejection: 'esd' }}
        onChange={() => {}}
      />
    );
    expect(screen.getByLabelText(/^Autocrop/)).toHaveValue('off');
    expect(screen.getByLabelText(/^Light rejection/)).toHaveValue('esd');
  });
});
