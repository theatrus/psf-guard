import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
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
    // Copying from a rig skips what the library already has.
    fireEvent.change(screen.getByLabelText('Copy templates from'), { target: { value: 'redcat' } });
    const copyList = await screen.findByRole('group', { name: 'Templates to copy' });
    expect(within(copyList).getByRole('button', { name: /Lum 90/ })).toBeDisabled();
    fireEvent.click(within(copyList).getByRole('button', { name: /Ha 300/ }));
    await waitFor(() => expect(saves).toHaveLength(3));
    expect(saves[2]).toMatchObject({ revision: 0, name: 'Ha 300', filter_name: 'Ha', gain: 100, default_exposure_seconds: 300 });
    // Removing sends the revision the row was read at.
    fireEvent.click(screen.getByRole('button', { name: 'Remove Lum 90' }));
    await waitFor(() => expect(deletes).toEqual(['11111111-1111-4111-8111-111111111111?3']));
    expect(await screen.findByText('Removed Lum 90.')).toBeInTheDocument();
  });

  it('is read only without write access', async () => {
    fixture(); mount(false);
    expect(await screen.findByDisplayValue('Lum 90')).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'New template' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Moon settings for Lum 90' }));
    expect(screen.getByLabelText('Enable Moon avoidance')).toBeDisabled();
  });
});
