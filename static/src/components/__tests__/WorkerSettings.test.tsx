import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import WorkerSettings from '../WorkerSettings';
import type { WorkersSettings } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const settings = (overrides: Partial<WorkersSettings> = {}): WorkersSettings => ({
  tokens: [{ id: 'abc', name: 'studio-pc', created_at: 1_760_000_000 }],
  workers: [
    {
      session: 'local',
      token_id: 'this-server',
      name: 'This server',
      version: '0.12.1',
      local: true,
      connected: true,
      connected_at: 1_760_000_000,
      seconds_since_seen: 0,
      threads: 16,
      rc_astro: [],
      file_mode: 'shared',
      exchange_shared: true,
    },
    {
      session: 's1',
      token_id: 'abc',
      name: 'studio-pc',
      version: '0.12.1',
      local: false,
      connected: true,
      connected_at: 1_760_000_000,
      seconds_since_seen: 3,
      threads: 32,
      rc_astro: ['RC-Astro BlurXTerminator 2.6.10'],
      file_mode: 'shared',
      exchange_shared: false,
      exchange_problem: 'no --map reaches the server\'s exchange folder',
      running: { label: 'RC-Astro BlurXTerminator on a 6248×4176 image', fraction: 0.42 },
    },
  ],
  queued: [{ label: 'RC-Astro NoiseXTerminator on a 100×100 image', needs: 'RC-Astro NoiseXTerminator 2.6.10' }],
  exchange_folder: '/cache/workers',
  default_exchange_folder: '/cache/workers',
  ...overrides,
});

const ok = <T,>(data: T) => ({ success: true, data, error: null });

describe('WorkerSettings', () => {
  it('lists each worker with what it runs and how its files travel', async () => {
    server.use(http.get('/api/workers', () => HttpResponse.json(ok(settings()))));
    render(<WorkerSettings />, { wrapper: wrapper() });
    const studio = await screen.findByText('studio-pc', { selector: '.worker-row strong' });
    const row = studio.closest('.worker-row') as HTMLElement;
    expect(within(row).getByText('RC-Astro BlurXTerminator 2.6.10')).toBeInTheDocument();
    expect(
      within(row).getByText(/Files go through the server: no --map reaches/),
    ).toBeInTheDocument();
    expect(within(row).getByText(/Running RC-Astro BlurXTerminator .* 42%/)).toBeInTheDocument();
    expect(screen.getByText('This server', { selector: '.user-source-badge' })).toBeInTheDocument();
    expect(screen.getByText(/needs RC-Astro NoiseXTerminator 2.6.10/)).toBeInTheDocument();
    expect(screen.getByText('In use')).toBeInTheDocument();
  });

  it('shows a new token once, inside the command to run', async () => {
    let created: unknown = null;
    server.use(
      http.get('/api/workers', () => HttpResponse.json(ok(settings({ tokens: [], workers: [] })))),
      http.post('/api/workers/tokens', async ({ request }) => {
        created = await request.json();
        return HttpResponse.json(ok({
          token: 'psfw_secret',
          summary: { id: 'n1', name: 'gpu-box', created_at: 1_760_000_000 },
          workers: settings({ tokens: [{ id: 'n1', name: 'gpu-box', created_at: 1_760_000_000 }], workers: [] }),
        }));
      }),
    );
    render(<WorkerSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: '+ Add a worker' }));
    fireEvent.change(screen.getByLabelText('Worker name'), { target: { value: ' gpu-box ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create token' }));
    const status = await screen.findByRole('status');
    expect(created).toEqual({ name: 'gpu-box' });
    expect(within(status).getByText(/PSF_GUARD_WORKER_TOKEN=psfw_secret psf-guard worker --server/))
      .toBeInTheDocument();
    fireEvent.click(within(status).getByRole('button', { name: 'Done' }));
    expect(screen.queryByText(/psfw_secret/)).not.toBeInTheDocument();
  });

  it('revokes a token after asking', async () => {
    let revoked = false;
    server.use(
      http.get('/api/workers', () => HttpResponse.json(ok(settings()))),
      http.delete('/api/workers/tokens/abc', () => {
        revoked = true;
        return HttpResponse.json(ok(settings({ tokens: [] })));
      }),
    );
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<WorkerSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke the token for studio-pc' }));
    await waitFor(() => expect(revoked).toBe(true));
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('its task goes back in the queue'));
    expect(await screen.findByText('No worker tokens yet.')).toBeInTheDocument();
    confirm.mockRestore();
  });

  it('saves an exchange folder and can go back to the default', async () => {
    const saved: unknown[] = [];
    server.use(
      http.get('/api/workers', () => HttpResponse.json(ok(settings()))),
      http.put('/api/workers/exchange', async ({ request }) => {
        const body = await request.json() as { folder: string | null };
        saved.push(body);
        return HttpResponse.json(ok(settings({
          exchange_folder: body.folder ?? '/cache/workers',
          exchange_folder_setting: body.folder ?? undefined,
        })));
      }),
    );
    render(<WorkerSettings />, { wrapper: wrapper() });
    const field = await screen.findByLabelText('Folder');
    fireEvent.change(field, { target: { value: '/mnt/nas/psf-exchange' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save folder' }));
    expect(await screen.findByText('Exchange folder saved.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Use the default' }));
    await waitFor(() => expect(saved).toEqual([
      { folder: '/mnt/nas/psf-exchange' },
      { folder: null },
    ]));
  });
});
