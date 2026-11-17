import { beforeEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import OfflineMapOffer from '../OfflineMapOffer';
import type { DirectorSurvey } from '../../../api/directorTypes';
import type { SkyMapsView } from '../../../api/types';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const online = { id: 'dss2_color', name: 'DSS2 colour', kind: 'broadband', bandpass: 'optical', attribution: 'CDS' } as DirectorSurvey;
const offline = { ...online, id: 'nina:FramingAssistantCache', name: 'DSS (offline)', offline: true } as DirectorSurvey;

function maps(install: SkyMapsView['install'] = { running: false, message: '' }): SkyMapsView {
  return {
    chosen: false, sets: [{ key: 'full', name: 'Whole sky, DSS', covers: '', bytes: 3526916102, room_needed: 0, installed: false }],
    installed: [], cutouts_bytes: 0, install,
  };
}

describe('Offline map offer', () => {
  beforeEach(() => window.localStorage.clear());

  it('offers the whole-sky map once, and starts nothing until Install is clicked', async () => {
    const installs: string[] = [];
    let install: SkyMapsView['install'] = { running: false, message: '' };
    server.use(
      http.get('/api/sky-maps', () => ok(maps(install))),
      http.post('/api/sky-maps/install', async ({ request }) => {
        installs.push(((await request.json()) as { set: string }).set);
        install = { running: true, set: 'full', message: 'Downloading…', bytes_completed: 1024 ** 3, bytes_total: 3526916102 };
        return ok(install);
      }),
    );
    render(<OfflineMapOffer surveys={[online]} canWrite />, { wrapper: wrapper() });
    const offer = await screen.findByTestId('offline-map-offer');
    await waitFor(() => expect(offer).toHaveTextContent('Install the whole-sky map (3.3 GB)?'));
    expect(installs).toEqual([]);
    await userEvent.setup().click(screen.getByRole('button', { name: 'Install' }));
    await waitFor(() => expect(installs).toEqual(['full']));
    await waitFor(() => expect(screen.getByTestId('offline-map-offer'))
      .toHaveTextContent('Installing the whole-sky map. 1.0 GB of 3.3 GB. You can keep working.'));
    // Remembered only once the map is in place.
    expect(window.localStorage.getItem('psf-guard.framing.offlineMapOffer')).toBeNull();
    install = { running: false, set: 'full', message: 'Installed', finished_at: 2 };
    await waitFor(() => expect(window.localStorage.getItem('psf-guard.framing.offlineMapOffer')).toBe('installed'), { timeout: 4000 });
  });

  it('is not shown again after Not now', async () => {
    server.use(http.get('/api/sky-maps', () => ok(maps())));
    const { unmount } = render(<OfflineMapOffer surveys={[online]} canWrite />, { wrapper: wrapper() });
    await userEvent.setup().click(await screen.findByRole('button', { name: 'Not now' }));
    expect(screen.queryByTestId('offline-map-offer')).toBeNull();
    unmount();
    render(<OfflineMapOffer surveys={[online]} canWrite />, { wrapper: wrapper() });
    expect(screen.queryByTestId('offline-map-offer')).toBeNull();
  });

  it('asks nothing when a map is installed, before the layers are known, or of a reader', () => {
    let asked = 0;
    server.use(http.get('/api/sky-maps', () => { asked += 1; return ok(maps()); }));
    for (const [surveys, canWrite] of [[[online, offline], true], [undefined, true], [[online], false]] as const) {
      const { unmount } = render(<OfflineMapOffer surveys={surveys as DirectorSurvey[] | undefined} canWrite={canWrite} />, { wrapper: wrapper() });
      expect(screen.queryByTestId('offline-map-offer')).toBeNull();
      unmount();
    }
    expect(asked).toBe(0);
  });

  it('shows why an install it started failed, with the way to Settings', async () => {
    let install: SkyMapsView['install'] = { running: false, message: '' };
    server.use(
      http.get('/api/sky-maps', () => ok(maps(install))),
      http.post('/api/sky-maps/install', () => {
        install = { running: false, set: 'full', message: 'The install failed: no room', error: 'no room', finished_at: 1 };
        return ok(install);
      }),
    );
    render(<OfflineMapOffer surveys={[online]} canWrite />, { wrapper: wrapper() });
    await userEvent.setup().click(await screen.findByRole('button', { name: 'Install' }));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('The install failed: no room');
    expect(screen.getByRole('button', { name: 'Open Offline sky maps' })).toBeInTheDocument();
    // A failure is not remembered: the offer comes back.
    expect(window.localStorage.getItem('psf-guard.framing.offlineMapOffer')).toBeNull();
    await userEvent.setup().click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('button', { name: 'Install' })).toBeInTheDocument();
  });

  it('does not take another set being installed for the whole-sky map', async () => {
    const installs: string[] = [];
    const nsns = { running: true, set: 'nsns-ohs', message: 'Downloading…' };
    server.use(
      http.get('/api/sky-maps', () => ok({ ...maps(nsns), sets: [
        ...maps().sets, { key: 'nsns-ohs', name: 'Northern sky narrowband, with stars', covers: '', bytes: 1, room_needed: 0, installed: false },
      ] })),
      http.post('/api/sky-maps/install', () => { installs.push('full'); return ok(nsns); }),
    );
    render(<OfflineMapOffer surveys={[online]} canWrite />, { wrapper: wrapper() });
    expect(await screen.findByText(/Northern sky narrowband, with stars is being installed now/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Install' })).toBeNull();
    expect(installs).toEqual([]);
    expect(window.localStorage.getItem('psf-guard.framing.offlineMapOffer')).toBeNull();
  });
});
