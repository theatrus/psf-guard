import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import StorageSettings from '../StorageSettings';

function wrapper(queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const GiB = 2 ** 30;
const folders = (stacksNext = '/cache') => [
  { kind: 'cache', path: '/cache', next_path: '/cache', source: 'server_config', chosen: null },
  {
    kind: 'stacks', path: '/cache', next_path: stacksNext,
    source: stacksNext === '/cache' ? 'default' : 'settings',
    chosen: stacksNext === '/cache' ? null : stacksNext,
  },
  { kind: 'calibration', path: '/cache', next_path: '/cache', source: 'default', chosen: null },
];

const settings = (
  limit: number,
  overLimit = false,
  stacksNext = '/cache',
  stackLimit: number | null = null,
) => ({
  success: true,
  error: null,
  data: {
    max_volume_percent: limit,
    stack_max_volume_percent: stackLimit,
    calibration_max_volume_percent: stackLimit === null ? null : limit,
    default_max_volume_percent: 90,
    min_max_volume_percent: 0,
    volumes: [{
      path: '/var/cache/psf-guard/askar', databases: ['Askar'],
      total_bytes: 1000 * GiB, used_bytes: 930 * GiB, used_percent: 93, max_percent: limit,
      preview_bytes: 40 * GiB, stack_bytes: 120 * GiB, calibration_bytes: 8 * GiB, other_bytes: 0,
      culled_files: 1200, freed_bytes: 3 * GiB, over_limit: overLimit, checked_unix: 1,
      kinds: ['cache', 'stacks', 'calibration'],
    }],
    folders: folders(stacksNext),
    folder_notes: [],
    can_choose_folders: true,
  },
});

function unifiedSaveServer(failWbppOnce = false) {
  let stored = settings(90).data;
  let pixinsight = { binary: '/opt/PixInsight', runs_dir: '/old/wbpp',
    storage: { max_volume_percent: 90, intermediate_days: 7, master_days: 90 } };
  const calls: { endpoint: string; body: Record<string, unknown> }[] = [];
  const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
  server.use(
    http.get('/api/settings/storage', () => ok(stored)),
    http.get('/api/settings/pixinsight', () => ok(pixinsight)),
    http.put('/api/settings/storage/folders', async ({ request }) => {
      const body = await request.json() as Record<string, string>;
      calls.push({ endpoint: 'folders', body });
      stored = { ...stored, folders: stored.folders.map((folder) => folder.kind === 'stacks'
        ? { ...folder, chosen: body.stack_dir, next_path: body.stack_dir } : folder) };
      return ok(stored);
    }),
    http.put('/api/settings/pixinsight', async ({ request }) => {
      const body = await request.json() as typeof pixinsight;
      calls.push({ endpoint: 'wbpp', body });
      if (failWbppOnce) {
        failWbppOnce = false;
        return HttpResponse.json({ success: false, error: 'WBPP settings could not be saved' }, { status: 500 });
      }
      pixinsight = body;
      return ok(pixinsight);
    }),
    http.put('/api/settings/storage', async ({ request }) => {
      const body = await request.json() as Record<string, number>;
      calls.push({ endpoint: 'limits', body });
      stored = { ...stored, ...body };
      return ok(stored);
    }),
  );
  return calls;
}

describe('StorageSettings', () => {
  it('prevents a poll started during saving from replacing the saved result', async () => {
    unifiedSaveServer();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    let releasePoll: ((value: unknown) => void) | undefined;
    let poll: Promise<unknown> | undefined;
    server.use(http.put('/api/settings/storage', () => {
      poll = client.fetchQuery({ queryKey: ['storage-settings'], queryFn: () =>
        new Promise((resolve) => { releasePoll = resolve; }) }).catch(() => null);
      return HttpResponse.json(settings(20));
    }));
    render(<StorageSettings canManage />, { wrapper: wrapper(client) });
    const slider = await screen.findByRole('slider', { name: /^Volume usage limit/ });
    fireEvent.change(slider, { target: { value: '20' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save storage settings' }));
    await waitFor(() => expect(client.getQueryData<{ max_volume_percent: number }>(['storage-settings'])?.max_volume_percent).toBe(20));
    await act(async () => {
      releasePoll?.(settings(90).data);
      await poll;
    });
    expect(client.getQueryData<{ max_volume_percent: number }>(['storage-settings'])?.max_volume_percent).toBe(20);
    expect(slider).toHaveValue('20');
  });

  it('keeps local edits while unedited settings follow a refreshed server response', async () => {
    unifiedSaveServer();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<StorageSettings canManage />, { wrapper: wrapper(client) });
    fireEvent.change(await screen.findByRole('slider', { name: /^Volume usage limit/ }), { target: { value: '20' } });
    fireEvent.change(screen.getByLabelText('WBPP storage folder'), { target: { value: '/local/wbpp' } });
    act(() => {
      client.setQueryData(['storage-settings'], settings(95, false, '/external/stacks').data);
      client.setQueryData(['pixinsight-settings'], { binary: '/opt/PixInsight', runs_dir: '/external/wbpp',
        storage: { max_volume_percent: 85, intermediate_days: 14, master_days: 120 } });
    });
    expect(screen.getByRole('slider', { name: /^Volume usage limit/ })).toHaveValue('20');
    expect(screen.getByLabelText('WBPP storage folder')).toHaveValue('/local/wbpp');
    await waitFor(() => expect(screen.getByLabelText('Stacks')).toHaveValue('/external/stacks'));
    expect(screen.getByRole('slider', { name: /WBPP volume/ })).toHaveValue('85');
    expect(screen.getByLabelText('WBPP intermediate retention, in days')).toHaveValue(14);
  });

  it('groups WBPP with the other limits and folders and saves every edit from one button', async () => {
    const calls = unifiedSaveServer();
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    const limit = await screen.findByRole('slider', { name: /^Volume usage limit/ });
    fireEvent.change(limit, { target: { value: '20' } });
    fireEvent.pointerUp(limit);
    fireEvent.blur(limit);
    fireEvent.change(screen.getByLabelText('Stacks'), { target: { value: '/new/stacks' } });
    const folderSection = screen.getByRole('heading', { name: 'Folders' }).parentElement!;
    fireEvent.change(within(folderSection).getByLabelText('WBPP storage folder'), { target: { value: '/new/wbpp' } });
    fireEvent.change(screen.getByRole('slider', { name: /WBPP volume/ }), { target: { value: '25' } });
    fireEvent.click(screen.getByRole('checkbox', { name: /Keep WBPP masters forever/ }));
    expect(calls).toEqual([]);
    expect(screen.queryByText(/Stacks, their processing, and WBPP runs/)).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /^Save/ })).toHaveLength(1);
    const save = screen.getByRole('button', { name: 'Save storage settings' });
    fireEvent.click(save);
    await waitFor(() => expect(save).toBeDisabled());
    await waitFor(() => expect(calls).toEqual([
      { endpoint: 'folders', body: { stack_dir: '/new/stacks' } },
      { endpoint: 'wbpp', body: { binary: '/opt/PixInsight', runs_dir: '/new/wbpp', storage: { max_volume_percent: 25, intermediate_days: 7, master_days: 0 } } },
      { endpoint: 'limits', body: { max_volume_percent: 20 } },
    ]));
    expect(screen.getByLabelText('Stacks')).toHaveValue('/new/stacks');
    expect(screen.getByLabelText('WBPP storage folder')).toHaveValue('/new/wbpp');
  });

  it('keeps unsaved edits after a partial failure and retries only the remaining settings', async () => {
    const calls = unifiedSaveServer(true);
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    fireEvent.change(await screen.findByLabelText('Stacks'), { target: { value: '/new/stacks' } });
    fireEvent.change(screen.getByLabelText('WBPP storage folder'), { target: { value: '/new/wbpp' } });
    fireEvent.change(screen.getByRole('slider', { name: /^Volume usage limit/ }), { target: { value: '20' } });
    const save = screen.getByRole('button', { name: 'Save storage settings' });
    fireEvent.click(save);
    expect(await screen.findByRole('alert')).toHaveTextContent('Some settings were saved');
    expect(screen.getByLabelText('WBPP storage folder')).toHaveValue('/new/wbpp');
    expect(screen.getByRole('slider', { name: /^Volume usage limit/ })).toHaveValue('20');
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.click(save);
    await waitFor(() => expect(calls.at(-1)?.endpoint).toBe('limits'));
    expect(calls.filter((call) => call.endpoint === 'folders')).toHaveLength(1);
    expect(calls.filter((call) => call.endpoint === 'wbpp')).toHaveLength(2);
    await waitFor(() => expect(save).toBeDisabled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows each cache volume against its limit and what it holds', async () => {
    server.use(http.get('/api/settings/storage', () => HttpResponse.json(settings(90, true))));
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByText('93.0% used of 1000.0 GiB')).toBeInTheDocument();
    expect(screen.getByRole('meter', { name: /Disk use of/ })).toHaveAttribute('aria-valuenow', '93');
    expect(screen.getByText(/Stacks 120.0 GiB · calibration masters 8.0 GiB · image previews 40.0 GiB/))
      .toBeInTheDocument();
    expect(screen.getByText(/deleted 1200 files, 3.0 GiB/)).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('PSF Guard preserves stacks');
  });

  it('stages a new limit until the shared Save action', async () => {
    let saved: unknown;
    server.use(
      http.get('/api/settings/storage', () => HttpResponse.json(settings(90))),
      http.put('/api/settings/storage', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(settings(80));
      })
    );
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    const slider = await screen.findByRole('slider', { name: /Volume usage limit/ });
    fireEvent.change(slider, { target: { value: '80' } });
    fireEvent.pointerUp(slider);
    expect(saved).toBeUndefined();
    fireEvent.click(screen.getByRole('button', { name: 'Save storage settings' }));
    await waitFor(() => expect(saved).toEqual({ max_volume_percent: 80 }));
  });

  it('allows volume limits all the way down to zero', async () => {
    let saved: unknown;
    server.use(
      http.get('/api/settings/storage', () => HttpResponse.json(settings(90))),
      http.put('/api/settings/storage', async ({ request }) => { saved = await request.json(); return HttpResponse.json(settings(0)); })
    );
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    const slider = await screen.findByRole('slider', { name: /Volume usage limit/ });
    expect(slider).toHaveAttribute('min', '0');
    fireEvent.change(slider, { target: { value: '0' } });
    fireEvent.pointerUp(slider);
    expect(saved).toBeUndefined();
    fireEvent.click(screen.getByRole('button', { name: 'Save storage settings' }));
    await waitFor(() => expect(saved).toEqual({ max_volume_percent: 0 }));
  });

  it('saves a stack folder for the next start and leaves a fixed cache alone', async () => {
    let saved: unknown;
    server.use(
      http.get('/api/settings/storage', () => HttpResponse.json(settings(90))),
      http.put('/api/settings/storage/folders', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(settings(90, false, '/mnt/stacks'));
      })
    );
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByLabelText('Cache')).toBeDisabled();
    expect(screen.getByText('Set by the server config file.')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Stacks'), { target: { value: ' /mnt/stacks ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save storage settings' }));
    await waitFor(() => expect(saved).toEqual({ stack_dir: '/mnt/stacks' }));
    expect(await screen.findByText(/Restart PSF Guard to use the new folders/)).toBeInTheDocument();
    expect(screen.getByText('/cache', { selector: 'code' })).toBeInTheDocument();
  });

  it('gives stacks and masters limits of their own when asked', async () => {
    const saved: unknown[] = [];
    let stackLimit: number | null = null;
    server.use(
      http.get('/api/settings/storage', () => HttpResponse.json(settings(90, false, '/cache', stackLimit))),
      http.put('/api/settings/storage', async ({ request }) => {
        const body = (await request.json()) as { stack_max_volume_percent?: number | null };
        saved.push(body);
        if (body.stack_max_volume_percent !== undefined) stackLimit = body.stack_max_volume_percent;
        return HttpResponse.json(settings(90, false, '/cache', stackLimit));
      })
    );
    render(<StorageSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByText(/used for cache, stacks and calibration masters/)).toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: /Stack volume/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('checkbox', { name: /Separate limits/ }));
    expect(saved).toEqual([]);
    const stack = await screen.findByRole('slider', { name: /Stack volume/ });
    fireEvent.change(stack, { target: { value: '97' } });
    fireEvent.pointerUp(stack);
    expect(saved).toEqual([]);
    fireEvent.click(screen.getByRole('button', { name: 'Save storage settings' }));
    await waitFor(() => expect(saved).toEqual([{ stack_max_volume_percent: 97, calibration_max_volume_percent: 90 }]));

  });

  it('shows the limit but cannot change it without database management', async () => {
    server.use(http.get('/api/settings/storage', () => HttpResponse.json(settings(90))));
    render(<StorageSettings canManage={false} />, { wrapper: wrapper() });
    expect(await screen.findByRole('slider', { name: /Volume usage limit/ })).toBeDisabled();
    expect(screen.getByText(/needs database management/)).toBeInTheDocument();
  });
});
