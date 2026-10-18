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
              files_total: 6,
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
    expect(screen.getByText('2/6 files')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute('value', '50');
  });

  function capabilities(gaia: Record<string, unknown>, ready: boolean) {
    return HttpResponse.json({
      success: true,
      data: {
        seiza_version: '0.12.0',
        seiza_fits_version: '0.2.0',
        resources: {
          objects: { name: 'objects', status: 'available' },
          stars: { name: 'stars', status: 'available' },
          star_identifiers: { name: 'star_identifiers', status: 'available' },
          blind_index: { name: 'blind_index', status: 'available' },
          transients: { name: 'transients', status: 'available' },
          minor_bodies: { name: 'minor_bodies', status: 'available' },
          gaia_photometry: gaia,
        },
        features: {
          object_association: true,
          object_name_search: false,
          stellar_name_search: false,
          hinted_solve: true,
          blind_solve: true,
          transient_annotations: false,
          minor_body_annotations: false,
          color_calibration: ready,
        },
      },
      error: null,
      status: 'ready',
    });
  }

  it('installs Gaia colors on their own, and a color card brings the person to them', async () => {
    let requestedPreset = '';
    server.use(
      http.get('/api/astrometry/capabilities', () =>
        capabilities({ name: 'gaia_photometry', status: 'not_configured' }, false)
      ),
      http.post('/api/astrometry/catalogs/install', async ({ request }) => {
        requestedPreset = ((await request.json()) as { preset: string }).preset;
        return HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true, phase: 'downloading', message: 'Downloading stars-gaia-photometry.bin…',
              preset: 'gaia_photometry', files_completed: 0, files_total: 1,
              bytes_completed: 100, bytes_total: 476,
            },
          },
          error: null,
          status: 'ready',
        });
      })
    );

    const user = userEvent.setup();
    render(<SeizaCatalogControls focus="gaia_photometry" />, { wrapper: wrapper() });

    expect(await screen.findByText('○ Color calibration')).toBeInTheDocument();
    const item = screen.getByRole('region', { name: 'Gaia colors' });
    expect(item).toHaveTextContent('Not installed');
    const install = screen.getByRole('button', { name: 'Install Gaia colors' });
    expect(install).toHaveFocus();
    // The solver package stays as it was: Gaia colors are their own item.
    expect(screen.getByLabelText('Catalog package')).toHaveValue('blind_deep');
    await user.click(install);
    expect(requestedPreset).toBe('gaia_photometry');
    expect(await screen.findByText('Downloading stars-gaia-photometry.bin…')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Install Gaia colors' })).toBeDisabled();
    expect(item).toHaveTextContent('Installing…');
  });

  it('shows Gaia colors as installed once they are', async () => {
    server.use(
      http.get('/api/astrometry/capabilities', () =>
        capabilities(
          { name: 'gaia_photometry', status: 'available', size_bytes: 476 * 1024 * 1024 },
          true
        )
      )
    );
    render(<SeizaCatalogControls />, { wrapper: wrapper() });
    expect(await screen.findByText('✓ Color calibration')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Gaia colors' })).toHaveTextContent('Installed · 476 MiB');
    expect(screen.getByRole('button', { name: 'Update Gaia colors' })).toBeEnabled();
  });
});
