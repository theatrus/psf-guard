import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import PillSelect from '../PillSelect';

function Kind() {
  const [kind, setKind] = useState('');
  return (
    <PillSelect label="Type" value={kind} onChange={(event) => setKind(event.target.value)}>
      <option value="">All</option>
      <option value="flat">Flat</option>
    </PillSelect>
  );
}

describe('PillSelect', () => {
  it('shows the chosen option in the pill and follows a change', () => {
    const { container } = render(<Kind />);
    const value = container.querySelector('.toolbar-pill-value');
    expect(value).toHaveTextContent('All');
    fireEvent.change(screen.getByLabelText('Type'), { target: { value: 'flat' } });
    expect(value).toHaveTextContent('Flat');
  });

  it('follows the value it is given and passes changes on', () => {
    const onChange = vi.fn();
    const { container, rerender } = render(
      <PillSelect label="Rig" value="a" onChange={onChange}>
        <option value="a">Alpha</option>
        <option value="b">Beta</option>
      </PillSelect>,
    );
    rerender(
      <PillSelect label="Rig" value="b" onChange={onChange}>
        <option value="a">Alpha</option>
        <option value="b">Beta</option>
      </PillSelect>,
    );
    expect(container.querySelector('.toolbar-pill-value')).toHaveTextContent('Beta');
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'a' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});
