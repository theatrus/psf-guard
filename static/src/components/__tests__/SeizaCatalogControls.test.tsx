import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import SeizaCatalogControls from '../SeizaCatalogControls';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { retry: false },
    },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe('SeizaCatalogControls', () => {
  it('shows installed features and starts the selected package', async () => {
    let requestedPreset = '';
    server.use(
      http.get('/api/astrometry/capabilities', () =>
        HttpResponse.json({
          success: true,
          data: {
            seiza_version: '0.12.0',
            seiza_fits_version: '0.2.0',
            data_dir: '/catalogs',
            resources: {
              objects: {
                name: 'objects',
                status: 'available',
                format: 'SEIZAOB4',
                size_bytes: 1024,
              },
              stars: { name: 'stars', status: 'missing', error: 'not installed' },
              star_identifiers: { name: 'star_identifiers', status: 'not_configured' },
              blind_index: { name: 'blind_index', status: 'missing' },
              transients: { name: 'transients', status: 'available', size_bytes: 2048 },
              minor_bodies: { name: 'minor_bodies', status: 'available', size_bytes: 4096 },
            },
            features: {
              object_association: true,
              object_name_search: false,
              stellar_name_search: false,
              hinted_solve: false,
              blind_solve: false,
              transient_annotations: false,
              minor_body_annotations: false,
            },
          },
          error: null,
          status: 'ready',
        })
      ),
      http.post('/api/astrometry/catalogs/install', async ({ request }) => {
        const body = (await request.json()) as { preset: string };
        requestedPreset = body.preset;
        return HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true,
              phase: 'downloading',
              message: 'Downloading stars-deep-gaia17.bin…',
              preset: body.preset,
              output_dir: '/catalogs',
              file_name: 'stars-deep-gaia17.bin',
              files_completed: 2,
              files_total: 5,
              bytes_completed: 50,
              bytes_total: 100,
            },
          },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(<SeizaCatalogControls />, { wrapper: wrapper() });

    expect(await screen.findByText('✓ Overlays')).toBeInTheDocument();
    expect(screen.getByText('SEIZAOB4 · 1.0 KiB')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Install / update catalogs' }));

    expect(requestedPreset).toBe('blind_deep');
    expect(
      await screen.findByText('Downloading stars-deep-gaia17.bin…')
    ).toBeInTheDocument();
    expect(screen.getByText('2/5 files')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute('value', '50');
  });
});
