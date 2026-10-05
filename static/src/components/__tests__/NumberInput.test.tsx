import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import NumberInput from '../NumberInput';

function Goal() {
  const [goal, setGoal] = useState(10);
  return <>
    <NumberInput aria-label="Goal" value={goal} onChange={event => setGoal(Number(event.target.value))} />
    <output aria-label="Saved goal">{goal}</output>
  </>;
}

describe('NumberInput', () => {
  it('lets a number be typed over the old one instead of snapping to 0', () => {
    render(<Goal />);
    const input = screen.getByRole('spinbutton', { name: 'Goal' });
    fireEvent.change(input, { target: { value: '' } });
    // Emptied, the field stays empty and the goal keeps its value.
    expect(input).toHaveValue(null);
    expect(screen.getByLabelText('Saved goal')).toHaveTextContent('10');
    fireEvent.change(input, { target: { value: '25' } });
    expect(input).toHaveValue(25);
    expect(screen.getByLabelText('Saved goal')).toHaveTextContent('25');
  });

  it('shows the committed value again when left empty', () => {
    render(<Goal />);
    const input = screen.getByRole('spinbutton', { name: 'Goal' });
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.blur(input);
    expect(input).toHaveValue(10);
  });
});
