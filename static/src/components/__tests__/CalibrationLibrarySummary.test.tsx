import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
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
});
