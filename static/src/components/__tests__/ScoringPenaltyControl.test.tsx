import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import ScoringPenaltyControl from '../ScoringPenaltyControl';
import ScoringDefaultsSettings from '../ScoringDefaultsSettings';
import {
  SCORING_DEFAULTS as DEFAULTS,
  scoringDefaults,
  setScoringDefaults,
} from '../../hooks/useScoringPreferences';

let search = '';
function LocationProbe() {
  search = useLocation().search;
  return null;
}

function renderControl(query = 'db=main&project=1&target=1') {
  const view = render(
    <MemoryRouter initialEntries={[`/sequence?${query}`]}>
      <ScoringPenaltyControl />
      <LocationProbe />
    </MemoryRouter>,
  );
  // Open the popover.
  fireEvent.click(screen.getByText(/^Scoring/));
  return view;
}

const link = () => new URLSearchParams(search);
const summary = () => document.querySelector('.scoring-penalty-control summary')!;
const control = () => document.querySelector('.scoring-penalty-control')!;
const maxHfr = () => screen.getByLabelText('Max HFR', { exact: false }) as HTMLInputElement;
const minStars = () => screen.getByLabelText('Min stars', { exact: false }) as HTMLInputElement;
const satellite = () => screen.getByLabelText('Satellite trails', { exact: false }) as HTMLInputElement;
const saveButton = () => screen.getByRole('button', { name: 'Save as default' });
const resetButton = () => screen.getByRole('button', { name: 'Reset to default' });

describe('the Sequence toolbar\'s Scoring control', () => {
  beforeEach(() => setScoringDefaults({ ...DEFAULTS }));
  afterEach(() => setScoringDefaults({ ...DEFAULTS }));

  it('starts from the saved defaults', () => {
    setScoringDefaults({ ...DEFAULTS, satellite: 0, hfrRejectAbove: 2.55 });
    renderControl();
    expect(maxHfr()).toHaveValue(2.55);
    expect(minStars()).toHaveValue(null);
    expect(satellite()).toHaveValue('0');
    expect(screen.getByText('off', { selector: '.penalty-value' })).toBeInTheDocument();
    // At the defaults: nothing to save or reset.
    expect(summary()).toHaveTextContent(/^Scoring$/);
    expect(control()).not.toHaveClass('is-set');
    expect(saveButton()).toHaveAttribute('aria-disabled', 'true');
    expect(resetButton()).toHaveAttribute('aria-disabled', 'true');
  });

  it('lets the link win, and says the view differs', () => {
    setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.55 });
    renderControl('db=main&maxHfr=4');
    expect(maxHfr()).toHaveValue(4);
    expect(summary()).toHaveTextContent('Scoring (changed)');
    expect(control()).toHaveClass('is-set');
    expect(maxHfr().closest('label')).toHaveClass('is-set');
    expect(maxHfr().closest('label')).toHaveAttribute('title', expect.stringContaining('Your default: 2.55.'));
    expect(saveButton()).toHaveAttribute('aria-disabled', 'false');
  });

  it('applies a change to this view only, naming every value', async () => {
    const user = userEvent.setup();
    setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 3 });
    renderControl();
    await user.type(minStars(), '505{Enter}');
    expect(link().get('minStars')).toBe('505');
    expect(link().get('maxHfr')).toBe('3');
    expect(link().get('penaltySatellite')).toBe('1');
    expect(link().get('db')).toBe('main');
    expect(minStars()).toHaveValue(505);
    expect(summary()).toHaveTextContent('Scoring (changed)');
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, hfrRejectAbove: 3 });
  });

  it('applies a slider on release', () => {
    renderControl();
    fireEvent.change(satellite(), { target: { value: '0' } });
    expect(link().has('penaltySatellite')).toBe(false);
    fireEvent.pointerUp(satellite());
    expect(link().get('penaltySatellite')).toBe('0');
    expect(summary()).toHaveTextContent('Scoring (changed)');
  });

  it('scales the HFR-rise and star-loss penalties like the others', () => {
    renderControl();
    const hfrRise = screen.getByLabelText('HFR rise', { exact: false }) as HTMLInputElement;
    const starLoss = screen.getByLabelText('Star loss', { exact: false }) as HTMLInputElement;
    expect(hfrRise).toHaveValue('1');
    fireEvent.change(hfrRise, { target: { value: '2' } });
    fireEvent.pointerUp(hfrRise);
    expect(link().get('penaltyHfrRise')).toBe('2');
    fireEvent.change(starLoss, { target: { value: '0' } });
    fireEvent.pointerUp(starLoss);
    expect(link().get('penaltyStarLoss')).toBe('0');
    expect(control()).toHaveTextContent('200%');
    expect(summary()).toHaveTextContent('Scoring (changed)');
  });

  it('refuses a fractional Min stars or a Max HFR past its cap, rather than changing it', async () => {
    const user = userEvent.setup();
    renderControl();
    await user.type(minStars(), '12.5{Enter}');
    expect(minStars()).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Enter a whole number from 1 to 1,000,000, or leave it empty.',
    );
    expect(link().has('minStars')).toBe(false);
    expect(saveButton()).toHaveAttribute('aria-disabled', 'true');
    // Save as default saves nothing it would refuse.
    await user.click(saveButton());
    expect(scoringDefaults()).toEqual(DEFAULTS);

    await user.clear(minStars());
    expect(minStars()).not.toHaveAttribute('aria-invalid');
    await user.type(maxHfr(), '150');
    await user.tab();
    expect(maxHfr()).toHaveValue(150);
    expect(maxHfr()).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('up to 100');
    expect(link().has('maxHfr')).toBe(false);
  });

  it('saves the view as the default, keeping focus in the popover', async () => {
    const user = userEvent.setup();
    renderControl('db=main&penaltySatellite=1&penaltyPointing=1&penaltyTemporal=0.5&maxHfr=off&minStars=off');
    await user.type(minStars(), '505');
    // Saving takes what is typed, without leaving the field first.
    await user.click(saveButton());
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, temporal: 0.5, starCountRejectBelow: 505 });
    expect(search).toBe('?db=main');
    expect(minStars()).toHaveValue(505);
    expect(summary()).toHaveTextContent(/^Scoring$/);
    expect(saveButton()).toHaveAttribute('aria-disabled', 'true');
    expect(saveButton()).toHaveFocus();
  });

  it('resets the view to the default, keeping focus in the popover', async () => {
    const user = userEvent.setup();
    setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 3 });
    renderControl('db=main&maxHfr=off&penaltyPointing=0');
    expect(maxHfr()).toHaveValue(null);
    await user.click(resetButton());
    expect(search).toBe('?db=main');
    expect(maxHfr()).toHaveValue(3);
    expect(summary()).toHaveTextContent(/^Scoring$/);
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, hfrRejectAbove: 3 });
    expect(resetButton()).toHaveFocus();
    // A press while there is nothing to reset does nothing.
    await user.click(resetButton());
    expect(search).toBe('?db=main');
  });

  it('follows defaults saved elsewhere', () => {
    renderControl();
    expect(maxHfr()).toHaveValue(null);
    fireEvent.click(summary());
    // Settings saves new defaults while the view is open.
    act(() => setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.2 }));
    fireEvent.click(summary());
    expect(maxHfr()).toHaveValue(2.2);
    expect(summary()).toHaveTextContent(/^Scoring$/);
  });

  it('saves only defaults Settings then opens as valid', async () => {
    const user = userEvent.setup();
    const view = renderControl();
    await user.type(minStars(), '12.5');
    await user.click(saveButton());
    await user.clear(minStars());
    await user.type(minStars(), '12');
    await user.click(saveButton());
    expect(scoringDefaults().starCountRejectBelow).toBe(12);
    view.unmount();

    render(<ScoringDefaultsSettings />);
    const field = screen.getByRole('spinbutton', { name: 'Min stars' });
    expect(field).toHaveValue(12);
    expect(field).toHaveAttribute('aria-invalid', 'false');
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
