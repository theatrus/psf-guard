import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
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

    render(<CalibrationLibrarySummary dbId="demo" />, { wrapper: wrapper() });

    expect(await screen.findByText(/42 frames · 3 cached masters/)).toBeInTheDocument();
    expect(screen.getByText('Scope · Camera')).toBeInTheDocument();
    expect(screen.getByText(/10 bias · 12 dark · 8 dark-flat · 12 flat/)).toBeInTheDocument();
  });

  it('opens a database-scoped frame manager', async () => {
    const summary = {
      schema_version: 1,
      frame_count: 1,
      master_count: 1,
      rigs: [
        {
          rig_uuid: 'rig-1',
          name: 'Scope · Camera',
          frame_count: 1,
          bias: 0,
          dark: 1,
          dark_flat: 0,
          flat: 0,
        },
      ],
    };
    server.use(
      http.get('/api/db/demo/calibrations/details', () =>
        HttpResponse.json({
          success: true,
          data: {
            summary,
            frames: [
              {
                frame_uuid: 'frame-1',
                rig_uuid: 'rig-1',
                kind: 'dark',
                source_path: '/calibration/dark-300s.fits',
                source_exists: true,
                camera: 'Camera',
                width: 3000,
                height: 2000,
                binning_x: 1,
                binning_y: 1,
                gain: 100,
                exposure_s: 300,
                camera_temp: -10,
              },
            ],
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/demo/calibrations', () =>
        HttpResponse.json({ success: true, data: summary, error: null, status: 'ready' })
      )
    );

    render(
      <CalibrationLibrarySummary dbId="demo" dbName="Demo DB" canManage />,
      { wrapper: wrapper() }
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Manage' }));

    expect(await screen.findByRole('dialog', { name: 'Calibration library' })).toBeInTheDocument();
    expect(await screen.findByText('dark-300s.fits')).toBeInTheDocument();
    expect(screen.getByText(/3000×2000 · 1×1 bin · gain 100/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Forget' })).toBeInTheDocument();
  });
});
