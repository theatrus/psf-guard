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
import { unsavedLabels } from '../../hooks/useUnsavedChanges';

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
      views.set(site.id, { site, profile: { site_id: site.id, revision: 0, location: null, horizon: null, forecast: null, updated_at_ms: 1 } });
      return HttpResponse.json(ok(site));
    }),
    http.get('/api/director/v1/sites/:id/profile', ({ params }) => HttpResponse.json(ok(views.get(params.id as string)))),
    http.put('/api/director/v1/sites/:id/profile', async ({ params, request }) => {
      const edit = await request.json() as DirectorSiteProfileEdit;
      saves.push(edit);
      const view = views.get(params.id as string)!;
      const next: DirectorSiteProfileView = { site: view.site, profile: { ...view.profile, revision: view.profile.revision + 1,
        location: edit.location ? { ...edit.location, reported_at_ms: 5 } : null, horizon: edit.horizon ? { ...edit.horizon, reported_at_ms: 5 } : null, forecast: edit.forecast ?? null } };
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

  it('keeps a site\'s unsaved edits while another is open, and says they are unsaved', async () => {
    fixture();
    mount();
    for (const name of ['Folsom', 'Starfront']) {
      fireEvent.change(await screen.findByLabelText('New site name'), { target: { value: name } });
      fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
      await screen.findByRole('region', { name: `Site ${name}` });
    }
    // Starfront opened last; go back to Folsom and give it a location.
    fireEvent.click(screen.getByRole('button', { name: 'Edit Folsom' }));
    const folsom = await screen.findByRole('region', { name: 'Site Folsom' });
    fireEvent.change(within(folsom).getByLabelText('Latitude'), { target: { value: '38.66' } });
    expect(within(folsom).getByText('Unsaved changes')).toBeInTheDocument();
    expect(unsavedLabels()).toEqual(['site Folsom']);

    fireEvent.click(screen.getByRole('button', { name: 'Edit Starfront' }));
    expect(screen.getByRole('region', { name: 'Site Folsom', hidden: true })).not.toBeVisible();
    expect(unsavedLabels()).toEqual(['site Folsom']);
    fireEvent.click(screen.getByRole('button', { name: 'Edit Folsom' }));
    expect(within(screen.getByRole('region', { name: 'Site Folsom' })).getByLabelText('Latitude')).toHaveValue(38.66);
  });

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
      forecast: null,
      time_zone: null,
    });
    expect(await within(card).findByText('Saved Backyard.')).toBeInTheDocument();

    const blobs: Blob[] = [];
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn((blob: Blob) => { blobs.push(blob); return 'blob:hrz'; }), revokeObjectURL: vi.fn() }));
    fireEvent.click(within(card).getByRole('button', { name: /Download .hrz/ }));
    expect(await blobs[0].text()).toBe('# Azimuth Altitude, degrees\n0 12\n180 30\n360 12\n');
  });

  it('turns a forecast on with its threshold, says what is sent, and fetches it now', async () => {
    const { saves } = fixture();
    const asks: string[] = [];
    const forecastView = (fetched: boolean) => ({
      site: { id: 'x', name: 'Backyard', revision: 1 },
      settings: { provider: 'met_norway', cloud_threshold_percent: 25 },
      attribution: 'Weather data from MET Norway (CC BY 4.0)',
      forecast: fetched ? { site_id: 'x', provider: 'met_norway', fetched_at_ms: Date.UTC(2026, 10, 3, 14), checked_at_ms: Date.UTC(2026, 10, 3, 15), expires_at_ms: Date.UTC(2026, 10, 4, 3), refresh_after_ms: null, last_modified: 'Tue, 03 Nov 2026 13:50:00 GMT',
        hours: [{ start_ms: Date.UTC(2026, 10, 3, 14), end_ms: Date.UTC(2026, 10, 12, 0), cloud_total: 10, cloud_low: null, cloud_mid: null, cloud_high: null }] } : null,
      fresh: fetched,
      status: { site_id: 'x', last_attempt_ms: Date.UTC(2026, 10, 3, 14), last_error: fetched ? null : 'PSF Guard could not reach MET Norway: timed out', failures: fetched ? 0 : 1, retry_after_ms: null },
      next_refresh_ms: Date.UTC(2026, 10, 3, 17),
    });
    server.use(
      http.get('/api/director/v1/sites/:id/forecast', () => HttpResponse.json(ok(forecastView(false)))),
      http.post('/api/director/v1/sites/:id/forecast/refresh', ({ params }) => { asks.push(params.id as string); return HttpResponse.json(ok(forecastView(true))); }),
    );
    mount();
    fireEvent.change(await screen.findByLabelText('New site name'), { target: { value: 'Backyard' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
    const card = await screen.findByRole('region', { name: 'Site Backyard' });
    expect(await within(card).findByText('Off. PSF Guard sends nothing about this site.')).toBeInTheDocument();
    fireEvent.change(within(card).getByLabelText('Forecast'), { target: { value: 'met_norway' } });
    expect(within(card).getByText(/sends MET Norway this site's latitude and longitude, rounded to about 1 km, and nothing else: when you save, when you click Fetch now, every three hours, and again after a failed request/)).toBeInTheDocument();
    // Turned back off after a threshold edit, nothing is left to save.
    fireEvent.change(within(card).getByLabelText('Clear at or under'), { target: { value: '50' } });
    fireEvent.change(within(card).getByLabelText('Forecast'), { target: { value: 'off' } });
    expect(within(card).queryByText('Unsaved changes')).toBeNull();
    fireEvent.change(within(card).getByLabelText('Forecast'), { target: { value: 'met_norway' } });
    fireEvent.change(within(card).getByLabelText('Clear at or under'), { target: { value: '30' } });
    // A forecast is for a place: none without a location.
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    expect(await within(card).findByRole('alert')).toHaveTextContent("A forecast needs the site's location");
    fireEvent.change(within(card).getByLabelText('Latitude'), { target: { value: '59.91' } });
    fireEvent.change(within(card).getByLabelText('Longitude (east +)'), { target: { value: '10.75' } });
    fireEvent.change(within(card).getByLabelText('Clear at or under'), { target: { value: '' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(within(card).getByRole('alert')).toHaveTextContent('as a whole percent from 0 to 100'));
    fireEvent.change(within(card).getByLabelText('Clear at or under'), { target: { value: '25' } });
    // The dew limit is off while empty, and whole degrees up to 20.
    expect(within(card).getByLabelText('Not clear within')).toHaveAttribute('placeholder', 'Off');
    for (const value of ['21', '1.5']) {
      fireEvent.change(within(card).getByLabelText('Not clear within'), { target: { value } });
      expect(within(card).getByLabelText('Not clear within')).toBeInvalid();
    }
    fireEvent.change(within(card).getByLabelText('Not clear within'), { target: { value: '' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].forecast).toEqual({ provider: 'met_norway', cloud_threshold_percent: 25 });

    const status = await within(card).findByLabelText('Forecast status');
    expect(within(status).getByRole('alert')).toHaveTextContent('failed: PSF Guard could not reach MET Norway: timed out');
    expect(within(status).getByRole('link', { name: 'Weather data from MET Norway (CC BY 4.0)' })).toHaveAttribute('href', 'https://api.met.no/');
    fireEvent.click(within(status).getByRole('button', { name: 'Fetch now' }));
    expect(await within(status).findByText(/MET Norway confirmed it unchanged at .*\. Night plans use it until/)).toBeInTheDocument();
    expect(within(status).queryByRole('alert')).toBeNull();
    expect(asks).toHaveLength(1);

    fireEvent.change(within(card).getByLabelText('Not clear within'), { target: { value: '2' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].forecast).toEqual({ provider: 'met_norway', cloud_threshold_percent: 25, dew_limit_celsius: 2 });
    expect(await within(card).findByLabelText('Not clear within')).toHaveValue(2);
  });

  it('saves the site\'s time zone, or none for the browser\'s', async () => {
    const { saves } = fixture();
    mount();
    fireEvent.change(await screen.findByLabelText('New site name'), { target: { value: 'Backyard' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add site' }));
    const card = await screen.findByRole('region', { name: 'Site Backyard' });
    const zone = await within(card).findByLabelText('Time zone');
    expect(zone).toHaveValue('');
    expect(zone).toHaveAccessibleDescription(/marks and overrides for rigs at this site use this time zone/);
    fireEvent.change(zone, { target: { value: 'Europe/Berlin' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].time_zone).toBe('Europe/Berlin');
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
