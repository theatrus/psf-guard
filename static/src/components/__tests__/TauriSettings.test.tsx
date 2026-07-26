import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import TauriSettings from '../TauriSettings';

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
    },
  });

  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe('TauriSettings import state', () => {
  it('hides catalog management on a read-only server', async () => {
    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(
      await screen.findByText('🔒 Database management is read-only')
    ).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Seiza Catalogs' })).not.toBeInTheDocument();
  });

  it('resumes a server-side import scan when settings opens', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'archive',
              name: 'Archive',
              database_path: '/tmp/archive.sqlite',
              image_directories: ['/images'],
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true,
              stage: 'scanning',
              image_dirs: ['/images'],
              total_files: 120,
              scanned_files: 37,
              outcome: null,
              started_at: 1,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(await screen.findByText('Scanning headers… 37/120')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Importing…' })).toBeDisabled();
  });

  it('shows database quality work as a separate background job', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'archive',
              name: 'Archive',
              database_path: '/tmp/archive.sqlite',
              image_directories: ['/images'],
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running: false,
              stage: '',
              image_dirs: ['/images'],
              total_files: 0,
              scanned_files: 0,
              outcome: null,
              started_at: null,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/analysis/quality-backfill', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true,
              force: false,
              total_targets: 4,
              processed_targets: 1,
              current_target_id: 42,
              started_at: 1,
              finished_at: null,
            },
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(
      await screen.findByText(/Analyzing quality in the background… 1\/4 targets/)
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Import' })).toBeEnabled();
  });

  it('edits the per-database remote image receiver without exposing its token', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'remote',
              name: 'Remote catalog',
              database_path: '/tmp/remote.sqlite',
              image_directories: ['/images/remote'],
              remote_image_upload: {
                enabled: true,
                image_directory: '/images/remote',
                token_configured: true,
              },
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/remote/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running: false,
              stage: '',
              image_dirs: [],
              total_files: 0,
              scanned_files: 0,
              outcome: null,
              started_at: null,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(await screen.findByText('Remote receive: /images/remote')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    expect(
      screen.getByRole('checkbox', { name: 'Accept remote image uploads' })
    ).toBeChecked();
    expect(screen.getByLabelText('Receive directory:')).toHaveValue('/images/remote');
    expect(screen.getByLabelText('Remote API key:')).toHaveAttribute(
      'placeholder',
      'Unchanged'
    );

    fireEvent.click(screen.getByRole('button', { name: 'Generate' }));
    const generatedToken = screen.getByLabelText('Remote API key:');
    expect(generatedToken).toHaveAttribute('type', 'text');
    expect((generatedToken as HTMLInputElement).value).toMatch(/^[0-9a-f]{64}$/);
    expect(
      screen.getByText('Copy this key now. It will not be shown again after saving.')
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy' })).toBeEnabled();
  });
});

describe('TauriSettings first run', () => {
  function serveEmptyRegistry() {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        })
      )
    );
  }

  it('offers importing images as well as opening an existing database', async () => {
    serveEmptyRegistry();

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(
      await screen.findByRole('button', { name: /New Database from Images/ })
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /Add Existing Database/ })
    ).toBeInTheDocument();

    // Nothing that needs an existing catalog should sit between the welcome
    // banner and those two buttons.
    expect(
      screen.queryByText('Add a catalog before syncing with a remote PSF Guard.')
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        'Add a second catalog to merge data or send planning and grades.'
      )
    ).not.toBeInTheDocument();
  });

  it('opens straight onto the create form when the caller asks for it', async () => {
    serveEmptyRegistry();

    render(
      <TauriSettings isOpen initialIntent="create" onClose={() => undefined} />,
      { wrapper: createWrapper() }
    );

    expect(
      await screen.findByRole('heading', { name: 'New Database from Images' })
    ).toBeInTheDocument();
    // The create flow bootstraps its own database, so it never asks for one.
    expect(screen.queryByText('N.I.N.A. Database File:')).not.toBeInTheDocument();
  });

  it('opens straight onto the add form when the caller asks for it', async () => {
    serveEmptyRegistry();

    render(
      <TauriSettings isOpen initialIntent="add" onClose={() => undefined} />,
      { wrapper: createWrapper() }
    );

    expect(
      await screen.findByRole('heading', { name: 'Add Database' })
    ).toBeInTheDocument();
    expect(screen.getByText('N.I.N.A. Database File:')).toBeInTheDocument();
  });
});
