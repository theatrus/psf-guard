import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ScoringDefaultsSettings from '../ScoringDefaultsSettings';
import ReviewPreferences from '../ReviewPreferences';
import {
  SCORING_DEFAULTS as DEFAULTS,
  scoringDefaults,
  setScoringDefaults,
} from '../../hooks/useScoringPreferences';

const NAMES = {
  satellite: 'Satellite trails (percent)',
  pointing: 'Pointing (percent)',
  temporal: 'Temporal anomalies (percent)',
  hfrRise: 'HFR rise (percent)',
  starLoss: 'Star loss (percent)',
  hfr: 'Max HFR (pixels)',
  stars: 'Min stars',
};

const field = (name: string) => screen.getByRole('spinbutton', { name }) as HTMLInputElement;
const save = () => screen.getByRole('button', { name: 'Save defaults' });
const reset = () => screen.getByRole('button', { name: 'Reset to built-in defaults' });

describe('Settings › Review › Sequence scoring', () => {
  beforeEach(() => setScoringDefaults({ ...DEFAULTS }));
  afterEach(() => setScoringDefaults({ ...DEFAULTS }));

  it('sits in the Review tab', () => {
    render(<ReviewPreferences />);
    expect(screen.getByRole('heading', { name: 'Sequence scoring' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Sequence scoring' })).toBeInTheDocument();
  });

  it('lists each penalty and limit with its saved default and the built-in one', () => {
    setScoringDefaults({ ...DEFAULTS, satellite: 0, pointing: 1.5, hfrRejectAbove: 2.55 });
    render(<ScoringDefaultsSettings />);
    expect(field(NAMES.satellite)).toHaveValue(0);
    expect(field(NAMES.pointing)).toHaveValue(150);
    expect(field(NAMES.temporal)).toHaveValue(100);
    expect(field(NAMES.hfr)).toHaveValue(2.55);
    expect(field(NAMES.stars)).toHaveValue(null);
    expect(screen.getByText(/pixel-confirmed trail.*Built in: 100%\./)).toBeInTheDocument();
    expect(screen.getAllByText(/Built in: off\./)).toHaveLength(2);
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(reset()).toHaveAttribute('aria-disabled', 'false');
  });

  it('names each field with its unit and describes it with its hint', () => {
    render(<ScoringDefaultsSettings />);
    expect(field(NAMES.satellite)).toHaveAccessibleDescription(
      /^The score hit for a pixel-confirmed trail\..*Built in: 100%\.$/,
    );
    expect(field(NAMES.hfr)).toHaveAccessibleDescription(/measured HFR.*Built in: off\.$/);
    expect(field(NAMES.stars)).not.toHaveAttribute('aria-label');
  });

  it('saves the HFR-rise and star-loss penalties', async () => {
    const user = userEvent.setup();
    render(<ScoringDefaultsSettings />);
    expect(field(NAMES.hfrRise)).toHaveValue(100);
    await user.clear(field(NAMES.hfrRise));
    await user.type(field(NAMES.hfrRise), '150');
    await user.clear(field(NAMES.starLoss));
    await user.type(field(NAMES.starLoss), '0');
    await user.click(save());
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, hfrRise: 1.5, starLoss: 0 });
  });

  it('saves what is edited, keeping focus on Save', async () => {
    const user = userEvent.setup();
    render(<ScoringDefaultsSettings />);
    await user.clear(field(NAMES.satellite));
    await user.type(field(NAMES.satellite), '30');
    await user.type(field(NAMES.stars), '505');
    expect(scoringDefaults()).toEqual(DEFAULTS);
    await user.click(save());
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, satellite: 0.3, starCountRejectBelow: 505 });
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(save()).toHaveFocus();
    // Pressing it again saves nothing new.
    await user.click(save());
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, satellite: 0.3, starCountRejectBelow: 505 });

    // Emptying a limit turns it off.
    await user.clear(field(NAMES.stars));
    await user.click(save());
    expect(scoringDefaults().starCountRejectBelow).toBeNull();
  });

  it.each([
    [NAMES.satellite, '250', 'Enter 0 to 200%, in steps of 10.'],
    [NAMES.satellite, '35', 'Enter 0 to 200%, in steps of 10.'],
    [NAMES.pointing, '', 'Enter 0 to 200%, in steps of 10.'],
    [NAMES.temporal, '-10', 'Enter 0 to 200%, in steps of 10.'],
    [NAMES.hfr, '0', 'Enter a number above 0 and up to 100, or leave it empty.'],
    [NAMES.hfr, '120', 'Enter a number above 0 and up to 100, or leave it empty.'],
    [NAMES.stars, '10.5', 'Enter a whole number from 1 to 1,000,000, or leave it empty.'],
    [NAMES.stars, '0', 'Enter a whole number from 1 to 1,000,000, or leave it empty.'],
  ])('refuses %s of "%s"', async (name, text, message) => {
    const user = userEvent.setup();
    render(<ScoringDefaultsSettings />);
    await user.clear(field(name));
    if (text) await user.type(field(name), text);
    expect(field(name)).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent(message);
    // The error joins the hint in what a screen reader hears.
    expect(field(name)).toHaveAccessibleDescription(expect.stringContaining(message));
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    await user.click(save());
    expect(scoringDefaults()).toEqual(DEFAULTS);
  });

  it('resets to the built-in defaults', async () => {
    const user = userEvent.setup();
    setScoringDefaults({ ...DEFAULTS, temporal: 0, hfrRejectAbove: 3, starCountRejectBelow: 40 });
    render(<ScoringDefaultsSettings />);
    await user.click(reset());
    expect(scoringDefaults()).toEqual(DEFAULTS);
    expect(field(NAMES.temporal)).toHaveValue(100);
    expect(field(NAMES.hfr)).toHaveValue(null);
    expect(field(NAMES.stars)).toHaveValue(null);
    expect(reset()).toHaveAttribute('aria-disabled', 'true');
    expect(reset()).toHaveFocus();
  });

  it('shows defaults saved elsewhere, such as from the toolbar', () => {
    render(<ScoringDefaultsSettings />);
    act(() => setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.2 }));
    expect(field(NAMES.hfr)).toHaveValue(2.2);
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('keeps what is being typed when newer defaults arrive, and offers them', async () => {
    const user = userEvent.setup();
    render(<ScoringDefaultsSettings />);
    await user.type(field(NAMES.stars), '505');
    // Another tab, or a sign-in, saves other defaults meanwhile.
    act(() => setScoringDefaults({ ...DEFAULTS, hfrRejectAbove: 2.2 }));
    expect(field(NAMES.stars)).toHaveValue(505);
    expect(field(NAMES.hfr)).toHaveValue(null);
    expect(screen.getByRole('status')).toHaveTextContent('Newer defaults were saved elsewhere');

    await user.click(screen.getByRole('button', { name: 'Show the newer defaults' }));
    expect(field(NAMES.hfr)).toHaveValue(2.2);
    expect(field(NAMES.stars)).toHaveValue(null);
    expect(screen.queryByRole('status')).toBeNull();

    // Or the person's own values win when they save.
    await user.type(field(NAMES.stars), '505');
    act(() => setScoringDefaults({ ...DEFAULTS, temporal: 0 }));
    await user.click(save());
    expect(scoringDefaults()).toEqual({ ...DEFAULTS, hfrRejectAbove: 2.2, starCountRejectBelow: 505 });
    expect(screen.queryByRole('status')).toBeNull();
  });
});
