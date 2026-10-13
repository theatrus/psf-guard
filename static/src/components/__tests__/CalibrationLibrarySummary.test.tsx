import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { server } from '../../test/msw-server';
import CalibrationLibrarySummary from '../CalibrationLibrarySummary';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe('CalibrationLibrarySummary', () => {
  it('shows the per-rig frame coverage', async () => {
    server.use(
      http.get('/api/db/demo/calibrations', () =>
        HttpResponse.json({
          success: true,
          data: {
            schema_version: 1,
            frame_count: 42,
            master_count: 3,
            rigs: [
              {
                rig_uuid: 'rig-1',
                name: 'Scope · Camera',
                profile_id: 'profile',
                telescope: 'Scope',
                camera: 'Camera',
                frame_count: 42,
                bias: 10,
                dark: 12,
                dark_flat: 8,
                flat: 12,
              },
            ],
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<MemoryRouter><CalibrationLibrarySummary dbId="demo" /></MemoryRouter>, { wrapper: wrapper() });

    expect(await screen.findByText(/42 frames · 3 cached masters/)).toBeInTheDocument();
    expect(screen.getByText('Scope · Camera')).toBeInTheDocument();
    expect(screen.getByText(/10 bias · 12 dark · 8 dark-flat · 12 flat/)).toBeInTheDocument();
  });

  it('opens the calibration page for its database and closes what holds it', async () => {
    server.use(
      http.get('/api/db/demo/calibrations', () =>
        HttpResponse.json({
          success: true,
          data: { schema_version: 9, frame_count: 1, master_count: 0, rigs: [] },
          error: null,
          status: 'ready',
        })
      )
    );
    const onOpen = vi.fn();
    function Location() {
      const location = useLocation();
      return <output aria-label="location">{location.pathname + location.search}</output>;
    }
    render(
      <MemoryRouter initialEntries={['/?db=other&project=4&plan=x']}>
        <CalibrationLibrarySummary dbId="demo" onOpen={onOpen} />
        <Location />
      </MemoryRouter>,
      { wrapper: wrapper() }
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Open Calibration' }));

    expect(onOpen).toHaveBeenCalledTimes(1);
    // Another database's project does not come along, nor Planning's keys.
    expect(screen.getByRole('status', { name: 'location' })).toHaveTextContent('/calibration?db=demo');
  });
});
