import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
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
              backfill_total: 0,
              backfill_done: 0,
              backfill_current_target: null,
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
});
