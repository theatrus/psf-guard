import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import RigExposureTemplates from '../director/RigExposureTemplates';
import type { ExposureTemplateDetails } from '../../api/types';

const ok = (data: unknown) => ({ success: true, data, error: null });

const ha: ExposureTemplateDetails = {
  id: 7, profile_id: 'p', name: 'Ha 300', filter_name: 'Ha', gain: -1, offset: -1, bin: 1, readout_mode: -1,
  twilight_level: 0, moon_avoidance_enabled: false, moon_avoidance_separation: 60, moon_avoidance_width: 7,
  maximum_humidity: 0, default_exposure: 300, moon_relax_scale: 0, moon_relax_max_altitude: 5,
  moon_relax_min_altitude: -15, moon_down_enabled: false, dither_every: -1, minutes_offset: 0, plan_count: 3,
};

function fixture(first: ExposureTemplateDetails = ha) {
  const saves: Array<Record<string, unknown>> = [];
  let rows = [first];
  server.use(
    http.get('/api/db/rig/exposure-templates', () => HttpResponse.json(ok(rows))),
    http.put('/api/db/rig/exposure-templates/:id', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>;
      saves.push(body);
      const saved = { ...ha, ...body, gain: body.gain ?? -1, offset: body.offset ?? -1, readout_mode: body.readout_mode ?? -1 } as ExposureTemplateDetails;
      rows = [saved];
      return HttpResponse.json(ok(saved));
    }),
  );
  return { saves };
}

function mount(canEdit = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><RigExposureTemplates slug="rig" canEdit={canEdit} /></QueryClientProvider>);
}

describe('Rig exposure templates', () => {
  it('saves Moon avoidance, twilight and gain on a rig template', async () => {
    const { saves } = fixture();
    mount();
    expect(screen.queryByRole('button', { name: 'Save Ha 300' })).not.toBeInTheDocument();
    fireEvent.change(await screen.findByLabelText('Gain of Ha 300'), { target: { value: '139' } });
    fireEvent.click(screen.getByRole('button', { name: 'Moon and twilight settings for Ha 300' }));
    fireEvent.change(screen.getByLabelText('Twilight for Ha 300'), { target: { value: '1' } });
    fireEvent.click(screen.getByLabelText('Enable Moon avoidance'));
    fireEvent.change(screen.getByLabelText(/Separation at full Moon/), { target: { value: '110' } });
    fireEvent.change(screen.getByLabelText(/Half-separation width/), { target: { value: '0' } });
    expect(screen.getByRole('button', { name: 'Save Ha 300' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/Half-separation width/), { target: { value: '10' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Ha 300' }));

    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({
      name: 'Ha 300', gain: 139, offset: null, readout_mode: null, bin: 1, twilight_level: 1,
      moon_avoidance_enabled: true, moon_avoidance_separation: 110, moon_avoidance_width: 10,
    });
    expect(saves[0]).not.toHaveProperty('filter_name');
    expect(await screen.findByText('Ha 300 saved')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save Ha 300' })).not.toBeInTheDocument();
  });

  it('shows the templates without edits when this server does not allow database management', async () => {
    fixture();
    mount(false);
    expect(await screen.findByLabelText('Gain of Ha 300')).toBeDisabled();
    expect(screen.getByText(/View only/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete Ha 300' })).not.toBeInTheDocument();
  });

  it('drops edits with Undo', async () => {
    fixture();
    mount();
    fireEvent.change(await screen.findByLabelText('Name of Ha 300'), { target: { value: 'Ha 240' } });
    fireEvent.click(screen.getByRole('button', { name: 'Drop changes to Ha 300' }));
    expect(screen.getByLabelText('Name of Ha 300')).toHaveValue('Ha 300');
  });

  it('saves a template whose Moon values are out of range while avoidance is off', async () => {
    const { saves } = fixture({ ...ha, moon_relax_min_altitude: 10, moon_relax_max_altitude: 5, moon_avoidance_width: 0 });
    mount();
    fireEvent.change(await screen.findByLabelText('Gain of Ha 300'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Ha 300' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ gain: 120, moon_avoidance_enabled: false, moon_relax_min_altitude: 10 });
  });

  it('explains on the page why a row cannot be saved', async () => {
    fixture();
    mount();
    fireEvent.change(await screen.findByLabelText('Gain of Ha 300'), { target: { value: '1.5' } });
    expect(screen.getByRole('button', { name: 'Save Ha 300' })).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('Ha 300: Gain, offset and readout mode must be whole numbers');
  });

  it('deletes a template no exposure plan uses, after asking', async () => {
    const spare = { ...ha, id: 8, name: 'OIII 300', filter_name: 'OIII', plan_count: 0 };
    let rows = [ha, spare];
    const deleted: string[] = [];
    server.use(
      http.get('/api/db/rig/exposure-templates', () => HttpResponse.json(ok(rows))),
      http.delete('/api/db/rig/exposure-templates/:id', ({ params }) => {
        deleted.push(String(params.id));
        rows = rows.filter(row => String(row.id) !== params.id);
        return HttpResponse.json(ok({ id: 8, name: 'OIII 300', synced: true }));
      }),
    );
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
    mount();
    expect(await screen.findByRole('button', { name: 'Delete Ha 300' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Delete Ha 300' })).toHaveAttribute('title', expect.stringMatching(/^3 exposure plans use this template/));
    fireEvent.click(screen.getByRole('button', { name: 'Delete OIII 300' }));
    expect(deleted).toEqual([]);
    fireEvent.click(screen.getByRole('button', { name: 'Delete OIII 300' }));
    expect(confirm).toHaveBeenLastCalledWith(expect.stringMatching(/^Delete OIII 300\? The next sync also deletes it in other PSF Guard catalogs, and in N\.I\.N\.A\./));
    expect(await screen.findByText('OIII 300 deleted')).toBeInTheDocument();
    expect(deleted).toEqual(['8']);
    expect(screen.queryByLabelText('Name of OIII 300')).not.toBeInTheDocument();
    confirm.mockRestore();
  });

  it('warns before deleting a template without a GUID that only this catalog loses it', async () => {
    fixture({ ...ha, plan_count: 0, syncable: false });
    server.use(http.delete('/api/db/rig/exposure-templates/:id', () => HttpResponse.json(ok({ id: 7, name: 'Ha 300', synced: false }))));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Delete Ha 300' }));
    expect(confirm).toHaveBeenCalledWith('Delete Ha 300? It has no GUID, so only this catalog loses it. N.I.N.A. and other catalogs keep their copy.');
    expect(await screen.findByText(/Ha 300 deleted here only\. It has no GUID/)).toBeInTheDocument();
    confirm.mockRestore();
  });

  it('shows the refusal when plans started using the template, and reloads the list', async () => {
    let planCount = 0;
    server.use(
      http.get('/api/db/rig/exposure-templates', () => HttpResponse.json(ok([{ ...ha, plan_count: planCount }]))),
      http.delete('/api/db/rig/exposure-templates/:id', () => {
        planCount = 1;
        return HttpResponse.json(
          { success: false, data: null, error: 'Ha 300 is used by 1 exposure plan; change or delete that plan first' }, { status: 409 });
      }),
    );
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Delete Ha 300' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Ha 300 is used by 1 exposure plan');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete Ha 300' })).toBeDisabled());
    expect(screen.getByText('1 plan')).toBeInTheDocument();
    confirm.mockRestore();
  });

  it('keeps an edit typed while the save runs', async () => {
    let finish: () => void = () => {};
    const held = new Promise<void>(resolve => { finish = resolve; });
    server.use(
      http.get('/api/db/rig/exposure-templates', () => HttpResponse.json(ok([ha]))),
      http.put('/api/db/rig/exposure-templates/:id', async ({ request }) => {
        const body = await request.json() as Record<string, unknown>;
        await held;
        return HttpResponse.json(ok({ ...ha, ...body, gain: body.gain ?? -1, offset: -1, readout_mode: -1 }));
      }),
    );
    mount();
    fireEvent.change(await screen.findByLabelText('Gain of Ha 300'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Ha 300' }));
    fireEvent.change(screen.getByLabelText('Offset of Ha 300'), { target: { value: '25' } });
    finish();
    expect(await screen.findByText('Ha 300 saved')).toBeInTheDocument();
    expect(screen.getByLabelText('Offset of Ha 300')).toHaveValue(25);
    expect(screen.getByRole('button', { name: 'Save Ha 300' })).toBeEnabled();
  });
});
