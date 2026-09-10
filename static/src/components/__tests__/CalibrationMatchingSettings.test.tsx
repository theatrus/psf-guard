import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import CalibrationMatchingSettings from '../CalibrationMatchingSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const current = (rotation: number | null, flatStarMasking = false) => ({
  success: true,
  data: {
    rotation_tolerance_deg: rotation,
    default_rotation_tolerance_deg: 2,
    external_masters: 'prefer',
    flat_star_masking: flatStarMasking,
  },
  error: null,
});

describe('CalibrationMatchingSettings', () => {
  it('shows the default as the placeholder, not as a configured value', async () => {
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null)))
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const input = await screen.findByLabelText('Rotation tolerance in degrees');
    expect(input).toHaveValue(null);
    expect(input).toHaveAttribute('placeholder', '2');
    expect(screen.getByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeChecked();
    // Nothing to save while the field matches what the server holds.
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('saves an override and reflects the server response', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null))),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(3.5));
      })
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const input = await screen.findByLabelText('Rotation tolerance in degrees');
    fireEvent.change(input, { target: { value: '3.5' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(saved).toEqual({
        rotation_tolerance_deg: 3.5,
        external_masters: 'prefer',
        flat_star_masking: false,
      })
    );
    // The response is the new truth; the button falls back to disabled.
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    );
  });

  it('saves the external-master policy on its own', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null))),
      http.put('/api/settings/calibration', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json({
          ...current(null),
          data: { ...current(null).data, external_masters: 'fallback' },
        });
      })
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const select = await screen.findByLabelText('Masters from other software');
    expect(select).toHaveValue('prefer');
    fireEvent.change(select, { target: { value: 'fallback' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(saved).toEqual({
        rotation_tolerance_deg: null,
        external_masters: 'fallback',
        flat_star_masking: false,
      })
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    );
  });

  it('refuses a value the server would reject, before sending it', async () => {
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null)))
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const input = await screen.findByLabelText('Rotation tolerance in degrees');
    fireEvent.change(input, { target: { value: '181' } });
    expect(
      screen.getByText('Enter a value between 0 and 180 degrees.')
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('saves masking on and off and reloads its saved state', async () => {
    let masking = false;
    const updates: unknown[] = [];
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(3.5, masking))),
      http.put('/api/settings/calibration', async ({ request }) => {
        const update = await request.json() as { flat_star_masking: boolean };
        updates.push(update);
        masking = update.flat_star_masking;
        return HttpResponse.json(current(3.5, masking));
      })
    );
    const first = render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(masking).toBe(true));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
    first.unmount();

    const second = render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const restored = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    await waitFor(() => expect(restored).toBeChecked());
    fireEvent.click(restored);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(masking).toBe(false));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
    second.unmount();

    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    expect(await screen.findByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeChecked();
    expect(updates).toEqual([true, false].map((enabled) => ({
      rotation_tolerance_deg: 3.5,
      external_masters: 'prefer',
      flat_star_masking: enabled,
    })));
  });

  it('keeps masking off when an older server omits the field', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json({
      success: true,
      data: {
        rotation_tolerance_deg: null,
        default_rotation_tolerance_deg: 2,
        external_masters: 'prefer',
      },
      error: null,
    })));
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    expect(await screen.findByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('retains an unsaved masking change after a failed save and allows retry', async () => {
    let attempts = 0;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null))),
      http.put('/api/settings/calibration', () => {
        attempts += 1;
        return HttpResponse.json(attempts === 1
          ? { success: false, data: null, error: 'Could not save the registry' }
          : current(null, true));
      })
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
    expect(toggle).toBeChecked();
    expect(attempts).toBe(2);
  });

  it('disables draft controls while saving to avoid losing a newer edit', async () => {
    let finishSave: () => void = () => {};
    const pending = new Promise<void>((resolve) => { finishSave = resolve; });
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current(null))),
      http.put('/api/settings/calibration', async () => {
        await pending;
        return HttpResponse.json(current(null, true));
      })
    );
    render(<CalibrationMatchingSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', { name: 'Mask stars in flats' });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(toggle).toBeDisabled());
    expect(screen.getByLabelText('Rotation tolerance in degrees')).toBeDisabled();
    expect(screen.getByLabelText('Masters from other software')).toBeDisabled();
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
