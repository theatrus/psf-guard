import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import RigProfileCard from '../director/RigProfileCard';
import type { DirectorRigProfile, DirectorRigProfileEdit, DirectorRigProfileView } from '../../api/directorTypes';
import { editFromForm, formFromProfile } from '../director/rigProfileForm';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'RedCat', revision: 1 };
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
  it('fills optics and site from frame headers, previews the field, and saves with the read revision', async () => {
    const { saves } = fixture(); mount();
    expect(await screen.findAllByText('Not set', { selector: '.rig-profile-source' })).toHaveLength(2);
    fireEvent.click(screen.getByRole('button', { name: 'Use frame headers (newest.fits)' }));
    expect(screen.getByLabelText('Sensor width')).toHaveValue(6248);
    expect(screen.getByTestId('rig-profile-fov')).toHaveTextContent('Field 5.38° × 3.60°, 3.10″ per pixel, f/4.9');
    expect(screen.getAllByText(/^From frame headers of newest\.fits/)[0]).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Use frame headers' }));
    expect(screen.getByLabelText('Latitude')).toHaveValue(34.2);
    fireEvent.change(screen.getByLabelText('Bortle class'), { target: { value: '6' } });
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
});
