import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import RigProfileCard from '../director/RigProfileCard';
import type { DirectorRigProfile, DirectorRigProfileEdit, DirectorRigProfileView } from '../../api/directorTypes';
import { editFromForm, formFromProfile } from '../director/rigProfileForm';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'RedCat', revision: 1 };
beforeEach(() => {
  server.use(
    http.get(`/api/director/v1/rigs/${rig.id}/collaboration`, () => HttpResponse.json(ok([]))),
    http.get('/api/director/v1/preferences', () => HttpResponse.json(ok({ global_id: 'g', presets: {}, sites: [] }))),
    http.get(`/api/director/v1/preferences/rig/${rig.id}`, () => HttpResponse.json(ok({ scope: 'rig', scope_id: rig.id, revision: 0, overrides: { weights: {} }, enabled: null, site_id: null }))),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([]))),
  );
});
const optics = { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 250, aperture_mm: 51, rotation: { mode: 'manual' as const, angle_degrees: 0 } };
const site = { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 };
const empty: DirectorRigProfile = {
  rig_id: rig.id, revision: 0, optics: null, site: null, horizon: null, sky_quality: null,
  limits: { value: { minimum_altitude_degrees: 20, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } }, source: { kind: 'manual' }, reported_at_ms: 1 },
  configuration: null, peer_id: null, updated_at_ms: 1,
};
function fixture(conflictOnce = false) {
  const saves: DirectorRigProfileEdit[] = [];
  let profile = empty;
  const view = (): DirectorRigProfileView => ({
    rig, profile, field_of_view: null,
    defaults: {
      optics: { value: optics, source: { kind: 'frame_headers', file_name: 'newest.fits' }, reported_at_ms: 2 },
      site: { value: site, source: { kind: 'frame_headers', file_name: 'newest.fits' }, reported_at_ms: 2 },
      field_of_view: null,
    },
  });
  server.use(
    http.get('/api/peers', () => HttpResponse.json(ok([{ id: 'obs', name: 'Observatory', base_url: 'https://obs.example', catalog_id: null, token_configured: true }]))),
    http.get('/api/director/v1/catalogs/catalog/rig/profile', () => HttpResponse.json(ok(view()))),
    http.put('/api/director/v1/catalogs/catalog/rig/profile', async ({ request }) => {
      const edit = await request.json() as DirectorRigProfileEdit;
      saves.push(edit);
      if (conflictOnce && saves.length === 1) return HttpResponse.json({ success: false, data: null, error: 'stale' }, { status: 409 });
      profile = { ...profile, revision: profile.revision + 1,
        optics: edit.optics ? { ...edit.optics, reported_at_ms: 3 } : null,
        site: edit.site ? { ...edit.site, reported_at_ms: 3 } : null,
        sky_quality: edit.sky_quality ? { ...edit.sky_quality, reported_at_ms: 3 } : null,
        limits: { ...edit.limits, reported_at_ms: 3 }, peer_id: edit.peer_id };
      return HttpResponse.json(ok({ ...view(), profile, field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 } }));
    }),
  );
  return { saves };
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<RigProfileCard slug="catalog" />, { wrapper: Wrapper });
}

describe('Rig profile card', () => {
  it('uses vertical keyboard navigation on the setup rail and horizontal navigation on narrow screens', async () => {
    let wide = true;
    let onChange = () => {};
    const remove = vi.fn();
    vi.stubGlobal('matchMedia', vi.fn(() => ({
      get matches() { return wide; },
      addEventListener: (_event: string, listener: () => void) => { onChange = listener; },
      removeEventListener: remove,
    })));
    let unmount = () => {};
    try {
      fixture(); ({ unmount } = mount());
      const opticsTab = await screen.findByRole('tab', { name: 'Optics' });
      const nav = screen.getByRole('tablist', { name: 'Rig setup sections' });
      expect(nav).toHaveAttribute('aria-orientation', 'vertical');
      opticsTab.focus();
      fireEvent.keyDown(opticsTab, { key: 'ArrowDown' });
      expect(screen.getByRole('tab', { name: 'Site' })).toHaveFocus();
      wide = false;
      act(() => onChange());
      expect(nav).toHaveAttribute('aria-orientation', 'horizontal');
      fireEvent.keyDown(screen.getByRole('tab', { name: 'Site' }), { key: 'ArrowRight' });
      expect(screen.getByRole('tab', { name: 'Limits and delivery' })).toHaveFocus();
    } finally { unmount(); vi.unstubAllGlobals(); }
    expect(remove).toHaveBeenCalledWith('change', onChange);
  });

  it('keeps edits across focused tabs and supports keyboard navigation', async () => {
    fixture(); mount();
    const opticsTab = await screen.findByRole('tab', { name: 'Optics' });
    fireEvent.change(screen.getByLabelText('Focal length'), { target: { value: '310' } });
    expect(screen.getByRole('tabpanel', { name: 'Optics' })).toBeVisible();
    expect(screen.queryByRole('tabpanel', { name: 'Site' })).not.toBeInTheDocument();
    opticsTab.focus();
    fireEvent.keyDown(opticsTab, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Site' })).toHaveFocus();
    expect(screen.getByRole('tabpanel', { name: 'Site' })).toBeVisible();
    fireEvent.click(opticsTab);
    expect(screen.getByLabelText('Focal length')).toHaveValue(310);
    fireEvent.keyDown(opticsTab, { key: 'End' });
    expect(screen.getByRole('tab', { name: 'Collaboration' })).toHaveFocus();
    expect(await screen.findByRole('button', { name: 'Connect server' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Save rig profile' })).not.toBeInTheDocument();
  });
  it('fills optics and site from frame headers, previews the field, and saves with the read revision', async () => {
    const { saves } = fixture(); mount();
    expect(await screen.findAllByText('Not set', { selector: '.rig-profile-source' })).toHaveLength(2);
    fireEvent.click(screen.getByRole('button', { name: 'Use frame headers (newest.fits)' }));
    expect(screen.getByLabelText('Sensor width')).toHaveValue(6248);
    expect(screen.getByTestId('rig-profile-fov')).toHaveTextContent('Field 5.38° × 3.60°, 3.10″ per pixel, f/4.9');
    expect(screen.getAllByText(/^From frame headers of newest\.fits/)[0]).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Site' }));
    fireEvent.click(screen.getByRole('button', { name: 'Use frame headers' }));
    expect(screen.getByLabelText('Latitude')).toHaveValue(34.2);
    fireEvent.change(screen.getByLabelText('Bortle class'), { target: { value: '6' } });
    fireEvent.click(screen.getByRole('tab', { name: 'Limits and delivery' }));
    fireEvent.change(screen.getByLabelText('Stop before meridian'), { target: { value: '10' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByText('Saved rig profile revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0].expected_revision).toBe(0);
    expect(saves[0].optics).toEqual({ value: optics, source: { kind: 'frame_headers', file_name: 'newest.fits' } });
    expect(saves[0].site?.value).toEqual(site);
    expect(saves[0].sky_quality?.value).toEqual({ bortle_class: 6, sqm_mag_per_arcsec2: null });
    expect(saves[0].limits.value.meridian_exclusion).toEqual({ before_ms: 600000, after_ms: 0 });
    // A hand edit after a header fill changes the source, and the next save uses the new revision.
    fireEvent.click(screen.getByRole('tab', { name: 'Optics' }));
    fireEvent.change(screen.getByLabelText('Focal length'), { target: { value: '260' } });
    expect(screen.getAllByText(/^Set by hand/)[0]).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].expected_revision).toBe(1);
    expect(saves[1].optics?.source).toEqual({ kind: 'manual' });
    expect(saves[1].optics?.value.focal_length_mm).toBe(260);
  });

  it('refuses a half-filled form before sending and stops after a lost race', async () => {
    fixture(true); mount();
    await screen.findByRole('button', { name: 'Save rig profile' });
    fireEvent.change(screen.getByLabelText('Focal length'), { target: { value: '250' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('sensor size, pixel size and focal length together');
    fireEvent.change(screen.getByLabelText('Focal length'), { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('This rig changed since you loaded it');
    expect(screen.getByRole('button', { name: 'Save rig profile' })).toBeDisabled();
  });

  it('names a registered peer the plans push to, and keeps an unregistered one visible', async () => {
    const { saves } = fixture(); mount();
    fireEvent.click(await screen.findByRole('tab', { name: 'Limits and delivery' }));
    const select = await screen.findByLabelText('Plans push to');
    await waitFor(() => expect(screen.getByRole('option', { name: 'Observatory' })).toBeInTheDocument());
    expect(select).toHaveValue('');
    fireEvent.change(select, { target: { value: 'obs' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByText('Saved rig profile revision 1.')).toBeInTheDocument();
    expect(saves[0].peer_id).toBe('obs');
    expect(saves[0].limits.value.minimum_altitude_degrees).toBe(20);
    fireEvent.change(select, { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].peer_id).toBeNull();
    // A peer the registry no longer has still shows on the form, so the operator sees what will not happen.
    const gone = formFromProfile({ ...empty, peer_id: 'old-site' });
    expect(gone.peerId).toBe('old-site');
    expect((editFromForm(gone, empty) as DirectorRigProfileEdit).peer_id).toBe('old-site');
  });

  it('shows values read only without a save button', async () => {
    fixture(); mount(false);
    expect(await screen.findByText('Read only')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save rig profile' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Use frame headers/ })).not.toBeInTheDocument();
  });

  it('keeps an existing horizon and reported source when the form resaves', () => {
    const profile: DirectorRigProfile = { ...empty, revision: 3,
      optics: { value: optics, source: { kind: 'plugin' }, reported_at_ms: 5 },
      horizon: { value: { mode: 'custom', points: [{ azimuth_degrees: 0, altitude_degrees: 12 }] }, source: { kind: 'plugin' }, reported_at_ms: 5 } };
    const edit = editFromForm(formFromProfile(profile), profile);
    expect(typeof edit).not.toBe('string');
    if (typeof edit === 'string') return;
    expect(edit.expected_revision).toBe(3);
    expect(edit.optics?.source).toEqual({ kind: 'plugin' });
    expect(edit.horizon?.value).toEqual({ mode: 'custom', points: [{ azimuth_degrees: 0, altitude_degrees: 12 }] });
  });

  it('takes its site from the planning site, and pastes a horizon of its own that wins', async () => {
    const { saves } = fixture();
    const settingsSaves: unknown[] = [];
    const backyard = { id: '33333333-3333-4333-8333-333333333333', name: 'Backyard', revision: 1 };
    const curve = { mode: 'custom' as const, points: [{ azimuth_degrees: 0, altitude_degrees: 15 }, { azimuth_degrees: 90, altitude_degrees: 25 }, { azimuth_degrees: 360, altitude_degrees: 15 }] };
    let rigSettings = { scope: 'rig', scope_id: rig.id, revision: 0, overrides: { weights: {} }, enabled: null, site_id: null as string | null };
    server.use(
      http.get('/api/director/v1/preferences', () => HttpResponse.json(ok({ global_id: 'g', presets: {}, sites: [backyard] }))),
      http.get(`/api/director/v1/preferences/rig/${rig.id}`, () => HttpResponse.json(ok(rigSettings))),
      http.put(`/api/director/v1/preferences/rig/${rig.id}`, async ({ request }) => {
        const body = await request.json() as typeof rigSettings;
        settingsSaves.push(body);
        rigSettings = { ...body, revision: body.revision + 1 };
        return HttpResponse.json(ok(rigSettings));
      }),
      http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ rig, catalog_slug: 'catalog', catalog_name: 'RedCat 61', profile: empty, field_of_view: null, default_exposure_seconds: { broadband: 120, narrowband: 300 },
        site: { site: rigSettings.site_id ? backyard : null, location: rigSettings.site_id ? site : null, location_from: rigSettings.site_id ? 'site' : 'none', horizon: { mode: 'fixed_minimum' }, horizon_from: 'none' } }]))),
      http.post('/api/director/v1/horizons/parse', async ({ request }) => {
        const { text } = await request.json() as { text: string };
        return text.includes('north')
          ? HttpResponse.json({ success: false, data: null, error: 'Horizon file not read: line 1 is not an azimuth and an altitude.' }, { status: 400 })
          : HttpResponse.json(ok(curve));
      }),
    );
    mount();
    expect(await screen.findByTestId('rig-site-origin')).toHaveTextContent('Planning uses no location yet and a flat horizon at the minimum altitude.');
    fireEvent.click(await screen.findByRole('tab', { name: 'Site' }));
    fireEvent.change(await screen.findByLabelText('Planning site'), { target: { value: backyard.id } });

    fireEvent.change(screen.getByLabelText('Rig horizon file text'), { target: { value: 'north 5' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use pasted horizon' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('line 1 is not an azimuth');
    fireEvent.change(screen.getByLabelText('Rig horizon file text'), { target: { value: '90 25\n' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use pasted horizon' }));
    await waitFor(() => expect(screen.getByTestId('horizon-summary')).toHaveTextContent('3 points, highest 25° at azimuth 90°, not saved yet.'));
    expect(screen.getByRole('button', { name: /Download .hrz/ })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].horizon).toEqual({ value: curve, source: { kind: 'manual' } });
    await waitFor(() => expect(settingsSaves).toHaveLength(1));
    expect(settingsSaves[0]).toMatchObject({ scope: 'rig', site_id: backyard.id, revision: 0 });
    await waitFor(() => expect(screen.getByTestId('rig-site-origin')).toHaveTextContent('the location from Backyard'));
  });

  it('keeps the saved profile when the site step fails, so a second Save does not conflict with it', async () => {
    const { saves } = fixture();
    const backyard = { id: '33333333-3333-4333-8333-333333333333', name: 'Backyard', revision: 1 };
    const rigSettings = { scope: 'rig', scope_id: rig.id, revision: 0, overrides: { weights: {} }, enabled: null, site_id: null as string | null };
    let settingsCalls = 0;
    server.use(
      http.get('/api/director/v1/preferences', () => HttpResponse.json(ok({ global_id: 'g', presets: {}, sites: [backyard] }))),
      http.get(`/api/director/v1/preferences/rig/${rig.id}`, () => HttpResponse.json(ok(rigSettings))),
      http.put(`/api/director/v1/preferences/rig/${rig.id}`, async ({ request }) => {
        settingsCalls += 1;
        if (settingsCalls === 1) return HttpResponse.json({ success: false, data: null, error: 'metadata is busy' }, { status: 500 });
        const body = await request.json() as typeof rigSettings;
        return HttpResponse.json(ok({ ...body, revision: body.revision + 1 }));
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('tab', { name: 'Site' }));
    fireEvent.change(await screen.findByLabelText('Planning site'), { target: { value: backyard.id } });
    fireEvent.change(screen.getByLabelText('Bortle class'), { target: { value: '5' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Profile saved, planning site not: metadata is busy');
    fireEvent.click(screen.getByRole('button', { name: 'Save rig profile' }));
    expect(await screen.findByText('Saved rig profile revision 2.')).toBeInTheDocument();
    // The second Save names the revision the first one wrote.
    expect(saves.map(save => save.expected_revision)).toEqual([0, 1]);
    expect(settingsCalls).toBe(2);
  });

  it('offers a Reload after a lost race', async () => {
    fixture(true); mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Save rig profile' }));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('This rig changed since you loaded it');
    fireEvent.click(within(alert).getByRole('button', { name: 'Reload' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save rig profile' })).toBeEnabled());
  });
});
