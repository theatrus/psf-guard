import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import StorageSettings from '../StorageSettings';

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const GiB = 2 ** 30;
const settings = (limit: number, overLimit = false) => ({
  success: true,
  error: null,
  data: {
    max_volume_percent: limit,
    default_max_volume_percent: 90,
    min_max_volume_percent: 50,
    volumes: [{
      path: '/var/cache/psf-guard/askar', databases: ['Askar'],
      total_bytes: 1000 * GiB, used_bytes: 930 * GiB, used_percent: 93, max_percent: limit,
      preview_bytes: 40 * GiB, stack_bytes: 120 * GiB, calibration_bytes: 8 * GiB, other_bytes: 0,
      culled_files: 1200, freed_bytes: 3 * GiB, over_limit: overLimit, checked_unix: 1,
    }],
  },
});

describe('StorageSettings', () => {
  it('shows each cache volume against its limit and what it holds', async () => {
    server.use(http.get('/api/settings/storage', () => HttpResponse.json(settings(90, true))));
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByText('93.0% used of 1000.0 GiB')).toBeInTheDocument();
    expect(screen.getByRole('meter', { name: /Disk use of/ })).toHaveAttribute('aria-valuenow', '93');
    expect(screen.getByText(/Stacks 120.0 GiB · calibration masters 8.0 GiB · image previews 40.0 GiB/))
      .toBeInTheDocument();
    expect(screen.getByText(/culled 1200 previews and checkpoints, 3.0 GiB/)).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Stacks are never culled');
  });

  it('saves a new limit when the slider is let go', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/storage', () => HttpResponse.json(settings(90))),
      http.put('/api/settings/storage', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(settings(80));
      })
    );
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    const slider = await screen.findByRole('slider', { name: /Most of the cache volume/ });
    fireEvent.change(slider, { target: { value: '80' } });
    fireEvent.pointerUp(slider);
    await waitFor(() => expect(saved).toEqual({ max_volume_percent: 80 }));
  });

  it('shows the limit but cannot change it without database management', async () => {
    server.use(http.get('/api/settings/storage', () => HttpResponse.json(settings(90))));
    render(<StorageSettings canManage={false} />, { wrapper: wrapper() });
    expect(await screen.findByRole('slider', { name: /Most of the cache volume/ })).toBeDisabled();
    expect(screen.getByText(/needs database management/)).toBeInTheDocument();
  });
});
