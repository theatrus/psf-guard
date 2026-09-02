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

const current = (rotation: number | null) => ({
  success: true,
  data: {
    rotation_tolerance_deg: rotation,
    default_rotation_tolerance_deg: 2,
    external_masters: 'prefer',
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
      expect(saved).toEqual({ rotation_tolerance_deg: 3.5, external_masters: 'prefer' })
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
      expect(saved).toEqual({ rotation_tolerance_deg: null, external_masters: 'fallback' })
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
});
