import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import WorkerSettings from '../WorkerSettings';
import { serverAddress } from '../../utils/workerCommand';
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
      slots: 1,
      rc_astro: [],
      file_mode: 'shared',
      exchange_shared: true,
      running: [],
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
      slots: 2,
      rc_astro: ['RC-Astro BlurXTerminator 2.6.10'],
      file_mode: 'shared',
      exchange_shared: false,
      exchange_problem: 'no --map reaches the server\'s shared folder',
      running: [{ label: 'RC-Astro BlurXTerminator on a 6248×4176 image', fraction: 0.42 }],
    },
  ],
  queued: [{ label: 'RC-Astro NoiseXTerminator on a 100×100 image', needs: 'RC-Astro NoiseXTerminator 2.6.10' }],
  exchange_folder: '/cache/workers',
  default_exchange_folder: '/cache/workers',
  exchange_subfolder: 'psf-guard-exchange',
  placement: { rc_astro: 'workers_first' },
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
      within(row).getByText(/Files transfer through the server: no --map reaches/),
    ).toBeInTheDocument();
    expect(within(row).getByText(/Running RC-Astro BlurXTerminator .* 42%/)).toBeInTheDocument();
    expect(within(row).getByText(/2 tasks at a time/)).toBeInTheDocument();
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
    expect(within(status).getByText(/^PSF_GUARD_WORKER_TOKEN=psfw_secret psf-guard worker --server/))
      .toBeInTheDocument();
    expect(within(status).getByText(/^\$env:PSF_GUARD_WORKER_TOKEN="psfw_secret"; psf-guard worker --server/))
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

  it('saves a shared folder and can go back to the default', async () => {
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
    const field = await screen.findByLabelText('Shared folder');
    fireEvent.change(field, { target: { value: '/mnt/nas/psf-exchange' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Shared folder saved.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Use the default' }));
    await waitFor(() => expect(saved).toEqual([
      { folder: '/mnt/nas/psf-exchange' },
      { folder: null },
    ]));
  });

  it('saves where RC-Astro tools and stack builds run, each on its own', async () => {
    const saved: unknown[] = [];
    let current = settings({ placement: { rc_astro: 'workers_first', stack: 'this_server' } });
    server.use(
      http.get('/api/workers', () => HttpResponse.json(ok(current))),
      http.put('/api/workers/placement', async ({ request }) => {
        const placement = await request.json() as WorkersSettings['placement'];
        saved.push(placement);
        current = settings({ placement });
        return HttpResponse.json(ok(current));
      }),
    );
    render(<WorkerSettings />, { wrapper: wrapper() });
    const tools = await screen.findByRole('group', { name: 'RC-Astro tools' });
    const stacks = screen.getByRole('group', { name: 'Stack builds' });
    // Stack builds stay on this server until someone chooses otherwise.
    expect(within(stacks).getByRole('radio', { name: /^This server only/ })).toBeChecked();
    fireEvent.click(within(tools).getByRole('radio', { name: /^Workers only/ }));
    await waitFor(() => expect(within(tools).getByRole('radio', { name: /^Workers only/ })).toBeChecked());
    fireEvent.click(within(stacks).getByRole('radio', { name: /^Workers first/ }));
    await waitFor(() => expect(saved).toEqual([
      { rc_astro: 'workers_only', stack: 'this_server' },
      { rc_astro: 'workers_only', stack: 'workers_first' },
    ]));
    expect(screen.getByText(/WBPP runs stay on this server/)).toBeInTheDocument();
  });

  it('says which workers stack builds, with their Seiza stacking version', async () => {
    const base = settings();
    server.use(http.get('/api/workers', () => HttpResponse.json(ok({
      ...base,
      workers: [{ ...base.workers[1], stacking_version: '0.50.0' }],
    }))));
    render(<WorkerSettings />, { wrapper: wrapper() });
    expect(await screen.findByText('Stack builds (Seiza stacking 0.50.0)')).toBeInTheDocument();
  });

  it('puts a placeholder in the command when this address only works here', () => {
    const at = (url: string) => {
      const parsed = new URL(url);
      return serverAddress({ protocol: parsed.protocol, hostname: parsed.hostname, host: parsed.host });
    };
    expect(at('http://192.168.1.10:3000/settings')).toBe('http://192.168.1.10:3000');
    expect(at('https://psf.example.org/')).toBe('https://psf.example.org');
    for (const local of ['http://localhost:3000/', 'http://127.0.0.1:3000/', 'http://tauri.localhost/', 'tauri://localhost/']) {
      expect(at(local)).toBe('http://THIS-SERVER:PORT');
    }
  });
});
