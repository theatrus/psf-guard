import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { CALIBRATION_RULE_FIELDS } from '../../test/calibrationRuleFields';
import CalibrationMatchingSettings from '../CalibrationMatchingSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const current = (rules: Record<string, unknown> = {}, flatStarMasking = false) => ({
  success: true,
  data: {
    rotation_tolerance_deg: (rules.rotation_tolerance_deg as number | undefined) ?? null,
    default_rotation_tolerance_deg: 2,
    external_masters: 'prefer',
    flat_star_masking: flatStarMasking,
    exclude_uncalibrated: false,
    fields: CALIBRATION_RULE_FIELDS,
    rules,
    rigs: [],
  },
  error: null,
});

/** The rules a save sends: every key, `null` for the default. */
const everyRule = (set: Record<string, unknown> = {}) =>
  Object.fromEntries(CALIBRATION_RULE_FIELDS.map((field) => [field.key, set[field.key] ?? null]));

describe('CalibrationMatchingSettings', () => {
  it('shows each default as the empty field\'s placeholder and nothing to save', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json(current())));
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const rotation = await screen.findByLabelText('Flat rotation tolerance');
    expect(rotation).toHaveValue(null);
    expect(rotation).toHaveAttribute('placeholder', '2');
    expect(screen.getByLabelText('Dark reach')).toHaveAttribute('placeholder', '183');
    expect(screen.getByLabelText('Dark choice')).toHaveValue('complete');
    expect(screen.getByLabelText('No matching flat')).toHaveValue('flag');
    for (const legend of ['Matching', 'Dark masters', 'Flat masters', 'Light checks', 'Stacks and exports']) {
      expect(screen.getByRole('group', { name: legend })).toBeInTheDocument();
    }
    expect(screen.getByText(/Nothing is rebuilt/)).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('saves a tolerance, refuses one out of range, and writes no default', async () => {
    let saved: Record<string, unknown> | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(current({ dark_reach_days: 90 }));
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const reach = await screen.findByLabelText('Dark reach');
    fireEvent.change(reach, { target: { value: '5000' } });
    expect(screen.getByText('Enter 1 to 3650 days.')).toBeInTheDocument();
    expect(reach).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    fireEvent.change(reach, { target: { value: '90' } });
    // The default, typed out, is still the default.
    fireEvent.change(screen.getByLabelText('Complete night'), { target: { value: '10' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(saved).toEqual({
        flat_star_masking: false,
        exclude_uncalibrated: false,
        rules: everyRule({ dark_reach_days: 90 }),
      }),
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
    expect(screen.getByLabelText('Dark reach')).toHaveValue(90);
  });

  it('makes a check advisory or turns it off', async () => {
    let saved: Record<string, unknown> | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(current({ check_no_flat: 'advisory', check_mixed_flats: 'off' }));
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const noFlat = await screen.findByLabelText('No matching flat');
    expect(within(noFlat).getAllByRole('option').map((option) => option.textContent)).toEqual([
      'Poor', 'Check only', 'Off',
    ]);
    // A check that only ever says "check" has no advisory step.
    const mixed = screen.getByLabelText('Flats from several nights');
    expect(within(mixed).getAllByRole('option').map((option) => option.textContent)).toEqual(['Check', 'Off']);
    fireEvent.change(noFlat, { target: { value: 'advisory' } });
    fireEvent.change(mixed, { target: { value: 'off' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect((saved as { rules: Record<string, unknown> } | null)?.rules).toEqual(
        everyRule({ check_no_flat: 'advisory', check_mixed_flats: 'off' }),
      ),
    );
  });

  it('edits a frame grade\'s limits in percent and saves them as fractions', async () => {
    let saved: Record<string, unknown> | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(current());
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByText('Frame grades'));
    const suspect = screen.getByLabelText('Flat light pattern: suspect at');
    const poor = screen.getByLabelText('Flat light pattern: poor at');
    expect(suspect).toHaveValue(2);
    expect(poor).toHaveValue(5);
    fireEvent.change(suspect, { target: { value: '12' } });
    expect(screen.getByText('A frame must turn suspect before it turns poor.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    fireEvent.change(suspect, { target: { value: '5' } });
    fireEvent.change(poor, { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect((saved as { rules: Record<string, unknown> } | null)?.rules).toEqual(
        everyRule({ grade_flat_illumination: { suspect: 0.05, poor: null } }),
      ),
    );
  });

  it('saves masking on and off and reloads its saved state', async () => {
    let masking = false;
    const updates: unknown[] = [];
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current({}, masking))),
      http.put('/api/settings/calibration', async ({ request }) => {
        const update = (await request.json()) as { flat_star_masking: boolean };
        updates.push(update.flat_star_masking);
        masking = update.flat_star_masking;
        return HttpResponse.json(current({}, masking));
      }),
    );
    const first = render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(masking).toBe(true));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
    first.unmount();

    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const restored = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    await waitFor(() => expect(restored).toBeChecked());
    fireEvent.click(restored);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(masking).toBe(false));
    expect(updates).toEqual([true, false]);
  });

  it('leaves out lights that cannot be calibrated only once turned on', async () => {
    let saved: Record<string, unknown> | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(current());
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const exclude = await screen.findByRole('checkbox', { name: /Leave out lights that can't be calibrated/ });
    expect(exclude).not.toBeChecked();
    // One setting for stacks and every export alike.
    expect(screen.getByText(/Stacks, WBPP runs and every export leave them out/)).toBeInTheDocument();
    fireEvent.click(exclude);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(saved).toMatchObject({ exclude_uncalibrated: true }));
  });

  it('retains an unsaved change after a failed save and allows retry', async () => {
    let attempts = 0;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', () => {
        attempts += 1;
        return HttpResponse.json(attempts === 1
          ? { success: false, data: null, error: 'Could not save the registry' }
          : current({}, true));
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not save the registry');
    expect(toggle).toBeChecked();
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
    expect(attempts).toBe(2);
  });

  it('disables draft controls while saving to avoid losing a newer edit', async () => {
    let finishSave: () => void = () => {};
    const pending = new Promise<void>((resolve) => { finishSave = resolve; });
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current())),
      http.put('/api/settings/calibration', async () => {
        await pending;
        return HttpResponse.json(current({}, true));
      }),
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(toggle).toBeDisabled());
    expect(screen.getByLabelText('Flat rotation tolerance')).toBeDisabled();
    expect(screen.getByLabelText('Masters from other software')).toBeDisabled();
    expect(screen.getByLabelText('No matching flat')).toBeDisabled();
    finishSave();
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
  });

  it('does not offer editable defaults when loading the settings fails', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json({
      success: false, data: null, error: 'Registry unavailable',
    })));
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load calibration settings.');
    expect(screen.queryByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  });
});
