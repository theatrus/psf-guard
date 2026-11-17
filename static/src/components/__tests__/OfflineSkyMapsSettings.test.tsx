import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import OfflineSkyMapsSettings from '../OfflineSkyMapsSettings';
import { gigabytes } from '../skyMapsShared';
import type { SkyMapsView } from '../../api/types';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const GB = 1024 ** 3;
const sets: SkyMapsView['sets'] = [
  { key: 'full', name: 'Whole sky, DSS', covers: 'The whole sky in colour', bytes: 3.3 * GB, room_needed: 7.1 * GB, installed: false },
  { key: 'nsns-ohs', name: 'Northern sky narrowband, with stars', covers: 'The northern sky in O III, Hα and S II',
    licence: 'CC BY-NC-SA 4.0, Northern Sky Narrowband Survey', bytes: 1.33 * GB, room_needed: 3.2 * GB, installed: false },
];
const dss = { id: 'nina:FramingAssistantCache', folder: 'FramingAssistantCache', name: 'DSS (offline)',
  attribution: 'N.I.N.A. offline sky map', tiles: 2051, bytes: 3.3 * GB, set: 'full', in_cache: false };

function view(change: Partial<SkyMapsView> = {}): SkyMapsView {
  return {
    folder: '/data/share/psf-guard/sky-maps', default_folder: '/data/share/psf-guard/sky-maps', chosen: false,
    free_bytes: 100 * GB, sets, installed: [], cutouts_bytes: 120 * 1024 ** 2,
    install: { running: false, message: '' }, ...change,
  };
}

/** Serve `current()` and record what the page asks for. */
function serve(current: () => SkyMapsView) {
  const asked: { installs: string[]; stops: number; removed: string[]; moved: string[]; folders: unknown[] } =
    { installs: [], stops: 0, removed: [], moved: [], folders: [] };
  server.use(
    http.get('/api/sky-maps', () => ok(current())),
    http.post('/api/sky-maps/install', async ({ request }) => {
      asked.installs.push(((await request.json()) as { set: string }).set);
      return ok({ running: true, set: 'full', message: 'Starting…' });
    }),
    http.post('/api/sky-maps/install/stop', () => { asked.stops += 1; return ok(true); }),
    http.delete('/api/sky-maps/installed/:folder', ({ params }) => { asked.removed.push(String(params.folder)); return ok(view()); }),
    http.post('/api/sky-maps/installed/:folder/move', ({ params }) => {
      asked.moved.push(String(params.folder));
      return ok(view({ installed: [dss] }));
    }),
    http.put('/api/settings/sky-maps', async ({ request }) => {
      asked.folders.push(((await request.json()) as { folder: unknown }).folder);
      return ok(view({ folder: '/maps', chosen: true }));
    }),
  );
  return asked;
}

describe('Offline sky maps settings', () => {
  it('lists the sets with sizes and terms, and downloads nothing until asked', async () => {
    const asked = serve(() => view());
    render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    const full = (await screen.findByText('Whole sky, DSS')).closest('.catalog-optional') as HTMLElement;
    expect(within(full).getByText(/3\.3 GB to download/)).toBeInTheDocument();
    expect(within(full).getByText('Not installed')).toBeInTheDocument();
    expect(screen.getByText('CC BY-NC-SA 4.0, Northern Sky Narrowband Survey')).toBeInTheDocument();
    expect(screen.getByText(/Maps are kept in/)).toHaveTextContent('/data/share/psf-guard/sky-maps. 100.0 GB free there.');
    expect(screen.getByText(/Survey pictures fetched online take 120 MB/)).toBeInTheDocument();
    expect(asked.installs).toEqual([]);

    await userEvent.setup().click(screen.getByRole('button', { name: 'Install Whole sky, DSS' }));
    await waitFor(() => expect(asked.installs).toEqual(['full']));
  });

  it('shows an install under way with its progress and stops it', async () => {
    const asked = serve(() => view({
      install: { running: true, set: 'full', message: 'Downloading…', bytes_completed: 1.1 * GB, bytes_total: 3.3 * GB },
    }));
    render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    const status = (await screen.findByText('Downloading…')).closest('[role="status"]') as HTMLElement;
    expect(status).toHaveTextContent('1.1 GB of 3.3 GB');
    // No second install can start while one runs.
    expect(screen.getByRole('button', { name: 'Install Whole sky, DSS' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Install Whole sky, DSS' })).toHaveTextContent('Installing…');
    await userEvent.setup().click(screen.getByRole('button', { name: 'Stop install' }));
    await waitFor(() => expect(asked.stops).toBe(1));
  });

  it('removes an installed map only after a yes, and says what it deletes', async () => {
    const asked = serve(() => view({ installed: [dss], sets: [{ ...sets[0], installed: true }, sets[1]] }));
    render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    const full = (await screen.findByText('Whole sky, DSS')).closest('.catalog-optional') as HTMLElement;
    expect(within(full).getByText('Installed')).toBeInTheDocument();
    expect(within(full).getByText(/3\.3 GB on disk, 2051 tiles/)).toBeInTheDocument();
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Remove DSS (offline)' }));
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('deletes its 3.3 GB of files'));
    expect(asked.removed).toEqual([]);
    await user.click(screen.getByRole('button', { name: 'Remove DSS (offline)' }));
    await waitFor(() => expect(asked.removed).toEqual(['FramingAssistantCache']));
  });

  it('offers to move a map left in the cache folder', async () => {
    const asked = serve(() => view({ installed: [{ ...dss, in_cache: true }], sets: [{ ...sets[0], installed: true }, sets[1]] }));
    render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByText(/In the cache folder, where earlier versions installed maps/)).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole('button', { name: 'Move DSS (offline) to the offline sky maps folder' }));
    await waitFor(() => expect(asked.moved).toEqual(['FramingAssistantCache']));
    await waitFor(() => expect(screen.queryByText(/In the cache folder/)).toBeNull());
  });

  it('warns when the folder has too little room, and lists a map that is not a published set', async () => {
    serve(() => view({
      free_bytes: 2 * GB,
      installed: [{ id: 'nina:Mine', folder: 'Mine', name: 'Mine', attribution: 'My own tiles', tiles: 12, bytes: 40 * 1024 ** 2, in_cache: false }],
    }));
    render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    expect(await screen.findByText(/Needs 7\.1 GB free while it installs; the folder’s volume has 2\.0 GB\./)).toBeInTheDocument();
    const mine = screen.getByText('My own tiles').closest('.catalog-optional') as HTMLElement;
    expect(within(mine).getByText(/Mine · 40 MB on disk, 12 tiles/)).toBeInTheDocument();
    expect(within(mine).getByRole('button', { name: 'Remove Mine' })).toBeInTheDocument();
  });

  it('saves another folder, and shows no folder or file controls to someone who may not manage the server', async () => {
    const asked = serve(() => view());
    const { unmount } = render(<OfflineSkyMapsSettings canManage />, { wrapper: wrapper() });
    const field = await screen.findByLabelText('Offline sky maps folder');
    const save = screen.getByRole('button', { name: 'Save folder' });
    expect(save).toBeDisabled();
    const user = userEvent.setup();
    await user.type(field, '/maps');
    await user.click(save);
    await waitFor(() => expect(asked.folders).toEqual(['/maps']));
    await waitFor(() => expect(screen.getByText(/Maps are kept in/)).toHaveTextContent('/maps'));
    unmount();

    serve(() => view({ folder: undefined, default_folder: undefined, installed: [dss], sets: [{ ...sets[0], installed: true }, sets[1]] }));
    render(<OfflineSkyMapsSettings canManage={false} />, { wrapper: wrapper() });
    await screen.findByText('Whole sky, DSS');
    expect(screen.queryByLabelText('Offline sky maps folder')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove DSS (offline)' })).toBeNull();
    expect(screen.queryByText(/Maps are kept in/)).toBeNull();
    // Installing is still offered: it takes no path from the browser.
    expect(screen.getByRole('button', { name: 'Install Whole sky, DSS again' })).toBeEnabled();
  });

  it('writes sizes in gigabytes, or megabytes under about one', () => {
    expect(gigabytes(3.3 * GB)).toBe('3.3 GB');
    expect(gigabytes(471106178)).toBe('449 MB');
    expect(gigabytes(0)).toBe('0 MB');
  });
});
