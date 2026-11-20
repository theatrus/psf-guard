import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import TemplateLibrary from '../director/TemplateLibrary';
import type { DirectorLibraryTemplate } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const redcat = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile: null, field_of_view: null, default_exposure_seconds: { broadband: 120, narrowband: 300 } };

function fixture() {
  const library: DirectorLibraryTemplate[] = [{ id: '11111111-1111-4111-8111-111111111111', revision: 2, name: 'Lum 90', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure_seconds: 90, updated_at_ms: 1, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } }];
  const saves: Array<Record<string, unknown>> = [];
  const deletes: string[] = [];
  server.use(
    http.get('/api/director/v1/templates', () => HttpResponse.json(ok(library))),
    http.put('/api/director/v1/templates/:id', async ({ request, params }) => {
      const body = await request.json() as Record<string, unknown>;
      expect(body).not.toHaveProperty('bandpass');
      saves.push(body);
      const saved = { ...body, revision: Number(body.revision) + 1, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } } as DirectorLibraryTemplate;
      const index = library.findIndex(t => t.id === params.id);
      if (index >= 0) library[index] = saved; else library.push(saved);
      return HttpResponse.json(ok(saved));
    }),
    http.delete('/api/director/v1/templates/:id', ({ params, request }) => {
      deletes.push(`${params.id}?${new URL(request.url).searchParams.get('revision')}`);
      return HttpResponse.json(ok({ deleted: true }));
    }),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([redcat]))),
    http.get('/api/director/v1/catalogs/redcat/templates', () => HttpResponse.json(ok({ catalog_slug: 'redcat', catalog_name: 'RedCat 61', rig: redcat.rig, templates: [
      { id: 1, guid: null, profile_id: 'p', name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: 300, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } },
      { id: 2, guid: null, profile_id: 'p', name: 'Lum 90', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: 90, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } },
    ] }))),
  );
  return { saves, deletes };
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<TemplateLibrary />, { wrapper: Wrapper });
}

describe('Template library', () => {
  it('saves Moon settings, validates thresholds and retains values when disabled', async () => {
    const { saves } = fixture(); mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Moon settings for Lum 90' }));
    fireEvent.click(screen.getByLabelText('Enable Moon avoidance'));
    fireEvent.change(screen.getByLabelText(/Separation at full Moon/), { target: { value: '95' } });
    fireEvent.click(screen.getByLabelText('Moon must be down'));
    fireEvent.change(screen.getByLabelText(/Half-separation width/), { target: { value: '0' } });
    expect(screen.getByRole('button', { name: 'Save Lum 90' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/Half-separation width/), { target: { value: '8' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Lum 90' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ moon: { enabled: true, separation_degrees: 95, width_days: 8, moon_down: true } });
    await screen.findByText('Saved Lum 90.');
    fireEvent.click(screen.getByLabelText('Enable Moon avoidance'));
    fireEvent.click(screen.getByRole('button', { name: 'Save Lum 90' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1]).toMatchObject({ moon: { enabled: false, separation_degrees: 95, width_days: 8, moon_down: true } });
  });
  it('lists, edits, adds, copies from a rig and removes templates, each with its revision', async () => {
    const { saves, deletes } = fixture(); mount();
    const name = await screen.findByDisplayValue('Lum 90');
    expect(screen.getByText('Luminance')).toBeInTheDocument();
    // Editing marks the row and saves it with the revision that was read.
    fireEvent.change(screen.getByLabelText('Template exposure seconds'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Lum 90' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ id: '11111111-1111-4111-8111-111111111111', revision: 2, default_exposure_seconds: 120 });
    expect(await screen.findByText('Saved Lum 90.')).toBeInTheDocument();
    expect(name).toBeInTheDocument();
    // A new row needs a name and a filter before it can be saved.
    fireEvent.click(screen.getByRole('button', { name: 'New template' }));
    const rows = screen.getAllByRole('row');
    const fresh = within(rows[rows.length - 1]);
    expect(fresh.getByRole('button', { name: 'Save template' })).toBeDisabled();
    fireEvent.change(fresh.getByLabelText('Template name'), { target: { value: 'OIII 600' } });
    fireEvent.change(fresh.getByLabelText('Template filter'), { target: { value: 'OIII' } });
    fireEvent.change(fresh.getByLabelText('Template exposure seconds'), { target: { value: '600' } });
    fireEvent.click(fresh.getByRole('button', { name: 'Save OIII 600' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1]).toMatchObject({ revision: 0, name: 'OIII 600', filter_name: 'OIII', default_exposure_seconds: 600 });
    // Copying from a rig: Lum 90 is in the library, but at 120 s since the
    // edit, so the rig's copy differs from it.
    fireEvent.change(screen.getByLabelText('Copy templates from'), { target: { value: 'redcat' } });
    const copyList = await screen.findByRole('group', { name: 'Templates to copy' });
    expect(await within(copyList).findByRole('button', { name: /Lum 90.*differs from the library/ })).toBeEnabled();
    fireEvent.click(within(copyList).getByRole('button', { name: /Ha 300/ }));
    await waitFor(() => expect(saves).toHaveLength(3));
    expect(saves[2]).toMatchObject({ revision: 0, name: 'Ha 300', filter_name: 'Ha', gain: 100, default_exposure_seconds: 300 });
    // Removing asks first, then sends the revision the row was read at.
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
    fireEvent.click(screen.getByRole('button', { name: 'Remove Lum 90' }));
    expect(confirm).toHaveBeenCalledWith('Remove Lum 90 from the library? Plans keep their copy.');
    expect(deletes).toEqual([]);
    fireEvent.click(screen.getByRole('button', { name: 'Remove Lum 90' }));
    await waitFor(() => expect(deletes).toEqual(['11111111-1111-4111-8111-111111111111?3']));
    expect(await screen.findByText('Removed Lum 90.')).toBeInTheDocument();
    confirm.mockRestore();
  });

  it('lists one profile, marks what the library has, and brings a differing template up to the rig', async () => {
    const { saves } = fixture();
    // The rig's Lum 90 at 120 s against the library's at 90 s, and its Ha 300
    // as the library would copy it; two templates of another profile.
    server.use(http.get('/api/director/v1/catalogs/redcat/templates', () => HttpResponse.json(ok({ catalog_slug: 'redcat', catalog_name: 'RedCat 61', rig: redcat.rig, other_profile_templates: 2, templates: [
      { id: 2, guid: null, profile_id: 'p', name: 'Lum 90', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: 120, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } },
      { id: 3, guid: null, profile_id: 'p', name: 'Lum 90 copy', filter_name: 'L', gain: 100, offset: 30, bin: null, readout_mode: null, default_exposure: 90, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } },
    ] }))));
    mount();
    await screen.findByDisplayValue('Lum 90');
    fireEvent.change(screen.getByLabelText('Copy templates from'), { target: { value: 'redcat' } });
    const copyList = await screen.findByRole('group', { name: 'Templates to copy' });
    expect(await within(copyList).findByText('2 templates from other N.I.N.A. profiles are not listed.')).toBeInTheDocument();
    expect(within(copyList).getByRole('button', { name: /Lum 90 copy/ })).toBeEnabled();
    const differs = within(copyList).getByRole('button', { name: /Lum 90 \(L, differs from the library\)/ });
    expect(differs).toHaveAttribute('title', "Replace the library's Lum 90 with this database's settings");
    // Replacing a shared library template asks first.
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
    fireEvent.click(differs);
    expect(confirm).toHaveBeenCalledWith("Replace the library's Lum 90 with this database's settings? Plans that use it change at their next activation.");
    expect(saves).toHaveLength(0);
    fireEvent.click(differs);
    await waitFor(() => expect(saves).toHaveLength(1));
    // The library's own row, at the revision read, takes the rig's settings.
    expect(saves[0]).toMatchObject({ id: '11111111-1111-4111-8111-111111111111', revision: 2, name: 'Lum 90', default_exposure_seconds: 120 });
    // Once saved, the two match.
    expect(await within(copyList).findByRole('button', { name: /Lum 90 \(L, in the library\)/ })).toBeDisabled();
    confirm.mockRestore();
  });

  it('takes a library template without Moon settings for the defaults, and one of several same-named rows', async () => {
    fixture();
    const moon = { enabled: false, separation_degrees: 60, width_days: 7, relax_degrees_per_degree: 0, relax_min_altitude_degrees: -15, relax_max_altitude_degrees: 5, moon_down: false };
    server.use(
      http.get('/api/director/v1/templates', () => HttpResponse.json(ok([
        { id: '11111111-1111-4111-8111-111111111111', revision: 2, name: 'Lum 90', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure_seconds: 90, updated_at_ms: 1, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } },
        { id: '22222222-2222-4222-8222-222222222222', revision: 1, name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure_seconds: 300, moon, updated_at_ms: 1, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } },
        { id: '33333333-3333-4333-8333-333333333333', revision: 1, name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 2, readout_mode: null, default_exposure_seconds: 300, moon, updated_at_ms: 1, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } },
      ]))),
      // The server sends every rig template with its Moon settings, keys in another order.
      http.get('/api/director/v1/catalogs/redcat/templates', () => HttpResponse.json(ok({ catalog_slug: 'redcat', catalog_name: 'RedCat 61', rig: redcat.rig, templates: [
        { id: 1, guid: null, profile_id: 'p', name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 2, readout_mode: null, default_exposure: 300, moon, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } },
        { id: 2, guid: null, profile_id: 'p', name: 'Lum 90', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: 90, moon: { moon_down: false, relax_max_altitude_degrees: 5, relax_min_altitude_degrees: -15, relax_degrees_per_degree: 0, width_days: 7, separation_degrees: 60, enabled: false }, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' } },
        { id: 3, guid: null, profile_id: 'p', name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 4, readout_mode: null, default_exposure: 300, moon, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' } },
      ] }))),
    );
    mount();
    await screen.findByDisplayValue('Lum 90');
    fireEvent.change(screen.getByLabelText('Copy templates from'), { target: { value: 'redcat' } });
    const copyList = await screen.findByRole('group', { name: 'Templates to copy' });
    // Lum 90 has no Moon settings in the library: the defaults, the rig's.
    expect(await within(copyList).findByRole('button', { name: /Lum 90 \(L, in the library\)/ })).toBeDisabled();
    // The bin 2 Ha 300 matches the library's second row, not its first.
    expect(within(copyList).getByRole('button', { name: /Ha 300 \(Ha, 2×2, in the library\)/ })).toBeDisabled();
    // A bin 4 Ha 300 matches neither of two rows: it is offered as a copy.
    expect(within(copyList).getByRole('button', { name: /Ha 300 \(Ha, 4×4\)/ })).toHaveAttribute('title', 'Copy Ha 300 into the library');
  });

  it('lets the exposure be cleared and typed over', async () => {
    fixture(); mount();
    const exposure = await screen.findByLabelText('Template exposure seconds');
    fireEvent.change(exposure, { target: { value: '' } });
    expect(exposure).toHaveValue(null);
    fireEvent.change(exposure, { target: { value: '45' } });
    expect(exposure).toHaveValue(45);
  });

  it('is read only without write access', async () => {
    fixture(); mount(false);
    expect(await screen.findByDisplayValue('Lum 90')).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'New template' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Moon settings for Lum 90' }));
    expect(screen.getByLabelText('Enable Moon avoidance')).toBeDisabled();
  });
});
