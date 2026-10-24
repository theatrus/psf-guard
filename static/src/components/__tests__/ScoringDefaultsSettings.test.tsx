import { describe, expect, it } from 'vitest';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ScoringDefaultsSettings from '../ScoringDefaultsSettings';
import ReviewPreferences from '../ReviewPreferences';
import { SCORING_DEFAULTS as DEFAULTS, type ScoringPreferences } from '../../hooks/useScoringPreferences';
import {
  deliverScoringDefaults,
  renderWithScoring,
  scoringServer,
  waitForScoringDefaults,
} from '../../test/scoringDefaults';

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

/** Render the form and wait for the server's defaults to fill it. */
async function renderSettings(options: { viewer?: boolean } = {}) {
  const view = renderWithScoring(<ScoringDefaultsSettings />, options);
  await waitForScoringDefaults(view.client);
  return view;
}

/** Wait for the server to hold `expected` and the form to be ready again. */
async function savedAs(expected: ScoringPreferences) {
  await waitFor(() => expect(scoringServer.get()).toEqual(expected));
  await screen.findByRole('button', { name: 'Save defaults' });
}

describe('Settings › Review › Sequence scoring', () => {
  it('sits in the Review tab', () => {
    renderWithScoring(<ReviewPreferences />);
    expect(screen.getByRole('heading', { name: 'Sequence scoring' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Sequence scoring' })).toBeInTheDocument();
  });

  it('lists each penalty and limit with the server\'s default and the built-in one', async () => {
    scoringServer.set({ ...DEFAULTS, satellite: 0, pointing: 1.5, hfrRejectAbove: 2.55 });
    await renderSettings();
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

  it('names each field with its unit and describes it with its hint', async () => {
    await renderSettings();
    expect(field(NAMES.satellite)).toHaveAccessibleDescription(
      /^The score penalty for a pixel-confirmed trail\..*Built in: 100%\.$/,
    );
    expect(field(NAMES.hfr)).toHaveAccessibleDescription(/measured HFR.*Built in: off\.$/);
    expect(field(NAMES.stars)).not.toHaveAttribute('aria-label');
  });

  it('saves the HFR-rise and star-loss penalties', async () => {
    const user = userEvent.setup();
    await renderSettings();
    expect(field(NAMES.hfrRise)).toHaveValue(100);
    await user.clear(field(NAMES.hfrRise));
    await user.type(field(NAMES.hfrRise), '150');
    await user.clear(field(NAMES.starLoss));
    await user.type(field(NAMES.starLoss), '0');
    await user.click(save());
    await savedAs({ ...DEFAULTS, hfrRise: 1.5, starLoss: 0 });
  });

  it('saves what is edited, keeping focus on Save', async () => {
    const user = userEvent.setup();
    await renderSettings();
    await user.clear(field(NAMES.satellite));
    await user.type(field(NAMES.satellite), '30');
    await user.type(field(NAMES.stars), '505');
    expect(scoringServer.saves).toEqual([]);
    await user.click(save());
    await savedAs({ ...DEFAULTS, satellite: 0.3, starCountRejectBelow: 505 });
    expect(field(NAMES.satellite)).toHaveValue(30);
    expect(field(NAMES.stars)).toHaveValue(505);
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(save()).toHaveFocus();
    // Pressing it again saves nothing new.
    await user.click(save());
    expect(scoringServer.saves).toHaveLength(1);

    // Emptying a limit turns it off.
    await user.clear(field(NAMES.stars));
    await user.click(save());
    await savedAs({ ...DEFAULTS, satellite: 0.3 });
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
    await renderSettings();
    await user.clear(field(name));
    if (text) await user.type(field(name), text);
    expect(field(name)).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent(message);
    // The error joins the hint in what a screen reader hears.
    expect(field(name)).toHaveAccessibleDescription(expect.stringContaining(message));
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    await user.click(save());
    expect(scoringServer.saves).toEqual([]);
  });

  it('resets to the built-in defaults', async () => {
    const user = userEvent.setup();
    scoringServer.set({ ...DEFAULTS, temporal: 0, hfrRejectAbove: 3, starCountRejectBelow: 40 });
    await renderSettings();
    expect(field(NAMES.temporal)).toHaveValue(0);
    await user.click(reset());
    await waitFor(() => expect(scoringServer.saves).toEqual([DEFAULTS]));
    await savedAs(DEFAULTS);
    expect(field(NAMES.temporal)).toHaveValue(100);
    expect(field(NAMES.hfr)).toHaveValue(null);
    expect(field(NAMES.stars)).toHaveValue(null);
    expect(reset()).toHaveAttribute('aria-disabled', 'true');
    expect(reset()).toHaveFocus();
  });

  it('shows defaults saved elsewhere, such as from the toolbar', async () => {
    const { client } = await renderSettings();
    act(() => deliverScoringDefaults(client, { ...DEFAULTS, hfrRejectAbove: 2.2 }));
    await waitFor(() => expect(field(NAMES.hfr)).toHaveValue(2.2));
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('keeps what is being typed when newer defaults arrive, and offers them', async () => {
    const user = userEvent.setup();
    const { client } = await renderSettings();
    await user.type(field(NAMES.stars), '505');
    // Another person, or another tab, saves other defaults meanwhile.
    act(() => deliverScoringDefaults(client, { ...DEFAULTS, hfrRejectAbove: 2.2 }));
    expect(await screen.findByRole('status')).toHaveTextContent('Newer defaults were saved elsewhere');
    expect(field(NAMES.stars)).toHaveValue(505);
    expect(field(NAMES.hfr)).toHaveValue(null);

    await user.click(screen.getByRole('button', { name: 'Show the newer defaults' }));
    expect(field(NAMES.hfr)).toHaveValue(2.2);
    expect(field(NAMES.stars)).toHaveValue(null);
    expect(screen.queryByRole('status')).toBeNull();

    // Or the person's own values win when they save.
    await user.type(field(NAMES.stars), '505');
    act(() => deliverScoringDefaults(client, { ...DEFAULTS, temporal: 0 }));
    await screen.findByRole('status');
    await user.click(save());
    await savedAs({ ...DEFAULTS, hfrRejectAbove: 2.2, starCountRejectBelow: 505 });
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('lets a viewer read the defaults but not change them', async () => {
    const user = userEvent.setup();
    scoringServer.set({ ...DEFAULTS, hfrRejectAbove: 3 });
    await renderSettings({ viewer: true });
    expect(screen.getByText('Only an editor can change the defaults.')).toBeInTheDocument();
    expect(field(NAMES.hfr)).toHaveValue(3);
    for (const name of Object.values(NAMES)) expect(field(name)).toBeDisabled();
    expect(save()).toHaveAttribute('aria-disabled', 'true');
    expect(reset()).toHaveAttribute('aria-disabled', 'true');
    await user.click(save());
    await user.click(reset());
    expect(scoringServer.saves).toEqual([]);
    expect(scoringServer.get()).toEqual({ ...DEFAULTS, hfrRejectAbove: 3 });
  });

  it.each([
    ['refuses it', { error: 'The settings file is read-only' }, 'The settings file is read-only'],
    ['fails', { status: 500 as const }, 'Internal error'],
  ])('explains a save the server %s, and keeps its defaults', async (_, failure, message) => {
    const user = userEvent.setup();
    scoringServer.set({ ...DEFAULTS, hfrRejectAbove: 3 });
    scoringServer.failure = failure;
    await renderSettings();
    await user.type(field(NAMES.stars), '505');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(`Could not save the defaults: ${message}`);
    expect(scoringServer.get()).toEqual({ ...DEFAULTS, hfrRejectAbove: 3 });
    // What the person typed stays, to save again.
    expect(field(NAMES.hfr)).toHaveValue(3);
    expect(field(NAMES.stars)).toHaveValue(505);
    expect(save()).toHaveAttribute('aria-disabled', 'false');
  });

  it('leaves the fields as they were when a reset fails', async () => {
    const user = userEvent.setup();
    scoringServer.set({ ...DEFAULTS, temporal: 0, hfrRejectAbove: 3 });
    scoringServer.failure = { error: 'The settings file is read-only' };
    await renderSettings();
    await user.click(reset());
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Could not save the defaults: The settings file is read-only',
    );
    expect(scoringServer.get()).toEqual({ ...DEFAULTS, temporal: 0, hfrRejectAbove: 3 });
    expect(field(NAMES.temporal)).toHaveValue(0);
    expect(field(NAMES.hfr)).toHaveValue(3);
    expect(reset()).toHaveAttribute('aria-disabled', 'false');
  });
});
