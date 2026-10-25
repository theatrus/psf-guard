import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import StackEmissionControls from '../StackEmissionControls';

describe('continuum controls', () => {
  it('clears an optional manual B/G ratio back to measured photometry', () => {
    const onChange = vi.fn();
    render(<StackEmissionControls value={{ mode: 'manual', lines: { oiii: { amount: 0.25, coefficients: [0.1, 0.2], reference_flux_ratios: [1, 1.4] } }, saturation: {}, positive_contribution: true, broadband_amount: 0 }} combination="emission" disabled={false} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('OIII stellar B/G flux ratio'), { target: { value: '' } });
    expect(onChange.mock.lastCall?.[0].lines.oiii.reference_flux_ratios).toEqual([]);
  });
});

it('can turn matching off after switching to manual weights', () => {
  const onChange = vi.fn();
  render(<StackEmissionControls value={{ mode: 'manual', lines: { ha: { amount: 0.25, coefficients: [0.1], reference_flux_ratios: [] } }, photometry: { psfMatching: {} }, positive_contribution: true, broadband_amount: 0 }} combination="emission" disabled={false} onChange={onChange} />);
  const matching = screen.getByLabelText('Match continuum PSFs');
  expect(matching).toBeEnabled();
  fireEvent.click(matching);
  expect(onChange.mock.lastCall?.[0].photometry.psfMatching).toBeNull();
});
