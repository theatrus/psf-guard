import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import DirectorSites from '../director/DirectorSites';
import type { DirectorSiteProfileEdit, DirectorSiteProfileView } from '../../api/directorTypes';
import { horizonToHrz } from '../director/horizonFile';

const ok = (data: unknown) => ({ success: true, data, error: null });
const curve = { mode: 'custom' as const, points: [{ azimuth_degrees: 0, altitude_degrees: 12 }, { azimuth_degrees: 180, altitude_degrees: 30 }, { azimuth_degrees: 360, altitude_degrees: 12 }] };

function fixture() {
  const sites: { id: string; name: string; revision: number }[] = [];
  const views = new Map<string, DirectorSiteProfileView>();
  const saves: DirectorSiteProfileEdit[] = [];
  const parsed: string[] = [];
  server.use(
    http.get('/api/director/v1/sites', () => HttpResponse.json(ok({ items: sites, next_after: null }))),
    http.post('/api/director/v1/sites', async ({ request }) => {
      const body = await request.json() as { id: string; name: string };
      const site = { ...body, revision: 1 };
      sites.push(site);
      views.set(site.id, { site, profile: { site_id: site.id, revision: 0, location: null, horizon: null, updated_at_ms: 1 } });
      return HttpResponse.json(ok(site));
    }),
    http.get('/api/director/v1/sites/:id/profile', ({ params }) => HttpResponse.json(ok(views.get(params.id as string)))),
    http.put('/api/director/v1/sites/:id/profile', async ({ params, request }) => {
      const edit = await request.json() as DirectorSiteProfileEdit;
      saves.push(edit);
      const view = views.get(params.id as string)!;
      const next: DirectorSiteProfileView = { site: view.site, profile: { ...view.profile, revision: view.profile.revision + 1,
        location: edit.location ? { ...edit.location, reported_at_ms: 5 } : null, horizon: edit.horizon ? { ...edit.horizon, reported_at_ms: 5 } : null } };
      views.set(view.site.id, next);
      return HttpResponse.json(ok(next));
    }),
    http.post('/api/director/v1/horizons/parse', async ({ request }) => {
      parsed.push((await request.json() as { text: string }).text);
      return HttpResponse.json(ok(curve));
    }),
  );
  return { saves, parsed };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite: true }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<DirectorSites />, { wrapper: Wrapper });
}

describe('Director sites', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('adds a site, reads an uploaded horizon file, saves the location with it, and downloads it back', async () => {
    const { saves, parsed } = fixture();
    mount();
    expect(await screen.findByText('No sites yet.')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('New site name'), { target: { value: 'Backyard' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
    const card = await screen.findByRole('region', { name: 'Site Backyard' });
    fireEvent.change(await within(card).findByLabelText('Latitude'), { target: { value: '34.2' } });
    fireEvent.change(within(card).getByLabelText('Longitude (east +)'), { target: { value: '-118.3' } });
    fireEvent.change(within(card).getByLabelText('Elevation'), { target: { value: '400' } });

    const file = new File(['0 12\n180 30\n'], 'backyard.hrz', { type: 'text/plain' });
    fireEvent.change(within(card).getByLabelText('Upload Site horizon file'), { target: { files: [file] } });
    await waitFor(() => expect(within(card).getByTestId('horizon-summary')).toHaveTextContent('3 points, highest 30° at azimuth 180°, not saved yet.'));
    expect(parsed).toEqual(['0 12\n180 30\n']);

    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toEqual({
      expected_revision: 0,
      location: { value: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 }, source: { kind: 'manual' } },
      horizon: { value: curve, source: { kind: 'manual' } },
    });
    expect(await within(card).findByText('Saved Backyard.')).toBeInTheDocument();

    const blobs: Blob[] = [];
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn((blob: Blob) => { blobs.push(blob); return 'blob:hrz'; }), revokeObjectURL: vi.fn() }));
    fireEvent.click(within(card).getByRole('button', { name: /Download .hrz/ }));
    expect(await blobs[0].text()).toBe('# Azimuth Altitude, degrees\n0 12\n180 30\n360 12\n');
  });

  it('refuses half a location before sending', async () => {
    const { saves } = fixture();
    mount();
    fireEvent.change(await screen.findByLabelText('New site name'), { target: { value: 'Ridge' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
    const card = await screen.findByRole('region', { name: 'Site Ridge' });
    fireEvent.change(await within(card).findByLabelText('Latitude'), { target: { value: '34' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    expect(await within(card).findByRole('alert')).toHaveTextContent('Enter both latitude and longitude');
    expect(saves).toHaveLength(0);
  });

  it('renames once when the profile step after a rename fails, and offers a Reload after a lost race', async () => {
    const { saves } = fixture();
    const renames: Array<{ expected_revision: number; name: string }> = [];
    let failProfile: false | 500 | 409 = 500;
    server.use(
      http.patch('/api/director/v1/sites/:id', async ({ params, request }) => {
        const body = await request.json() as { expected_revision: number; name: string };
        renames.push(body);
        if (body.expected_revision !== 1) return HttpResponse.json({ success: false, data: null, error: 'stale' }, { status: 409 });
        return HttpResponse.json(ok({ id: params.id, name: body.name, revision: 2 }));
      }),
      http.put('/api/director/v1/sites/:id/profile', () => {
        if (failProfile) { const status = failProfile; failProfile = false; return HttpResponse.json({ success: false, data: null, error: status === 500 ? 'metadata is busy' : 'stale' }, { status }); }
        return undefined;
      }),
    );
    mount();
    fireEvent.change(await screen.findByLabelText('New site name'), { target: { value: 'Ridge' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
    const card = await screen.findByRole('region', { name: 'Site Ridge' });
    fireEvent.change(await within(card).findByLabelText('Site name'), { target: { value: 'High ridge' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    expect(await within(card).findByRole('alert')).toHaveTextContent('metadata is busy');
    // The rename went through; Save again sends only the profile.
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    expect(await within(card).findByText('Saved High ridge.')).toBeInTheDocument();
    expect(renames).toEqual([{ expected_revision: 1, name: 'High ridge' }]);
    expect(saves).toHaveLength(1);
    // Someone else saved the site first: say so, with a way back.
    failProfile = 409;
    fireEvent.change(within(card).getByLabelText('Latitude'), { target: { value: '34' } });
    fireEvent.change(within(card).getByLabelText('Longitude (east +)'), { target: { value: '-118' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    const alert = await within(card).findByRole('alert');
    expect(alert).toHaveTextContent('This site changed since you loaded it.');
    fireEvent.click(within(alert).getByRole('button', { name: 'Reload' }));
    await waitFor(() => expect(within(card).getByRole('button', { name: 'Save site' })).toBeEnabled());
  });

  it('writes a flat horizon as no file', () => {
    expect(horizonToHrz({ mode: 'fixed_minimum' })).toBeNull();
    expect(horizonToHrz(curve)).toBe('# Azimuth Altitude, degrees\n0 12\n180 30\n360 12\n');
  });
});
