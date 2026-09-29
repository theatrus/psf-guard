import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import PlanEditor from '../director/PlanEditor';
import type { DirectorPlanDraft } from '../../api/directorTypes';
import { convertGoal, framesFor, rigTotals } from '../director/planModel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const redcat = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile: null,
  field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const c925 = { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925', revision: 1 }, catalog_slug: 'c925', catalog_name: 'C925 data', profile: null, field_of_view: null, default_exposure_seconds: { broadband: 60, narrowband: 180 } };
const template = (id: number, name: string, filter: string, bandpass: string, kind: 'broadband' | 'narrowband', exposure: number) =>
  ({ id, guid: `00000000-0000-4000-8000-${String(id).padStart(12, '0')}`, profile_id: 'p', name, filter_name: filter, gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: exposure, bandpass: { id: bandpass, name, kind } });

const libraryHa = { id: '11111111-1111-4111-8111-111111111111', revision: 1, name: 'Ha 600 shared', filter_name: 'Ha', gain: 200, offset: 50, bin: 2, readout_mode: null, default_exposure_seconds: 600, updated_at_ms: 1, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' as const } };

function fixture(existing: DirectorPlanDraft | null = null, mosaic: { rows: number; columns: number; overlap_percent: number } | null = null, library = [libraryHa]) {
  const saves: DirectorPlanDraft[] = [];
  let plan = existing;
  server.use(
    http.get('/api/director/v1/templates', () => HttpResponse.json(ok(library))),
    http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, draft: mosaic ? {
      project_id: 'project', revision: 1, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.45 }, position_angle_degrees: 0, mosaic, panel_rig_id: null,
      panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 4, updated_at_ms: 1 } : null }))),
    http.get('/api/director/v1/projects/project/plan', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, plan }))),
    http.put('/api/director/v1/projects/project/plan', async ({ request }) => {
      const body = await request.json() as DirectorPlanDraft;
      saves.push(body);
      plan = { ...body, revision: body.revision + 1, updated_at_ms: 9 };
      return HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, plan }));
    }),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([redcat, c925]))),
    http.get('/api/director/v1/catalogs/redcat/templates', () => HttpResponse.json(ok({ catalog_slug: 'redcat', catalog_name: 'RedCat 61', rig: redcat.rig, templates: [
      template(1, 'Ha 300', 'Ha', 'h_alpha', 'narrowband', 300), template(2, 'Ha short', 'H-alpha', 'h_alpha', 'narrowband', 0), template(3, 'Lum', 'L', 'luminance', 'broadband', 90),
    ] }))),
    http.get('/api/director/v1/catalogs/c925/templates', () => HttpResponse.json(ok({ catalog_slug: 'c925', catalog_name: 'C925 data', rig: c925.rig, templates: [
      template(7, 'Red', 'R', 'red', 'broadband', 120),
    ] }))),
  );
  return { saves };
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<PlanEditor projectId="project" />, { wrapper: Wrapper });
}

describe('Plan editor', () => {
  it('binds a rig whose database lacks a template to a library template, by the library GUID and settings', async () => {
    const { saves } = fixture(); mount();
    await screen.findByText(/3 templates/);
    fireEvent.click(screen.getByRole('button', { name: 'Add objective' }));
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'h_alpha' } });
    // The C925 database has only a red template; the H-alpha objective falls back to the library.
    fireEvent.click(screen.getByRole('checkbox', { name: /C925 data/ }));
    const control = await screen.findByLabelText('C925 data template for H-alpha');
    expect(control).toHaveValue('lib:11111111-1111-4111-8111-111111111111');
    expect(screen.getByRole('group', { name: 'Library, written on activation' })).toBeInTheDocument();
    expect(screen.getByLabelText('C925 data exposure for H-alpha')).toHaveValue(600);
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].contributions[0]).toMatchObject({ rig_id: c925.rig.id, exposure_seconds: 600,
      template: { template_guid: '11111111-1111-4111-8111-111111111111', template_id: null, name: 'Ha 600 shared', filter_name: 'Ha', gain: 200, offset: 50, bin: 2, readout_mode: null } });
    // The RedCat database has its own H-alpha template and keeps it; the library is still on offer beside it.
    fireEvent.click(screen.getByRole('checkbox', { name: /RedCat 61/ }));
    const redcatControl = await screen.findByLabelText('RedCat 61 template for H-alpha');
    expect(redcatControl).toHaveValue('db:1');
    fireEvent.change(redcatControl, { target: { value: 'lib:11111111-1111-4111-8111-111111111111' } });
    expect(screen.getByLabelText('RedCat 61 template for H-alpha')).toHaveValue('lib:11111111-1111-4111-8111-111111111111');
  });

  it('adds an objective in hours, binds a rig through its matching template, and saves frames per rig', async () => {
    const { saves } = fixture(null, null, []); mount();
    expect(await screen.findByText('No objectives yet.')).toBeInTheDocument();
    await screen.findByText(/3 templates/);
    fireEvent.click(screen.getByRole('button', { name: 'Add objective' }));
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'h_alpha' } });
    fireEvent.change(screen.getByLabelText('Objective goal'), { target: { value: '6' } });
    fireEvent.click(screen.getByRole('checkbox', { name: /RedCat 61/ }));
    // The template's own default exposure wins; 6 h at 300 s is 72 frames.
    expect(screen.getByLabelText('RedCat 61 template for H-alpha')).toHaveValue('db:1');
    expect(screen.getByLabelText('RedCat 61 exposure for H-alpha')).toHaveValue(300);
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('72');
    expect(screen.getByText('72 frames, 6.0 h')).toBeInTheDocument();
    // Switching the unit keeps the goal: 6 h at the rig's 300 s is 72 frames, and back again 6 h.
    fireEvent.change(screen.getByLabelText('Objective goal unit'), { target: { value: 'frames' } });
    expect(screen.getByLabelText('Objective goal')).toHaveValue(72);
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('72');
    fireEvent.change(screen.getByLabelText('Objective goal unit'), { target: { value: 'hours' } });
    expect(screen.getByLabelText('Objective goal')).toHaveValue(6);
    expect(convertGoal({ kind: 'hours', value: 1 }, 'frames', 300)).toEqual({ kind: 'frames', value: 12 });
    expect(convertGoal({ kind: 'frames', value: 10 }, 'hours', 300)).toEqual({ kind: 'hours', value: 0.83 });
    expect(convertGoal({ kind: 'hours', value: 2 }, 'hours', 300)).toEqual({ kind: 'hours', value: 2 });
    // A template without a default falls back to the rig's narrowband default.
    fireEvent.change(screen.getByLabelText('RedCat 61 template for H-alpha'), { target: { value: 'db:2' } });
    expect(screen.getByLabelText('RedCat 61 exposure for H-alpha')).toHaveValue(300);
    fireEvent.change(screen.getByLabelText('RedCat 61 exposure for H-alpha'), { target: { value: '600' } });
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('36');
    // The other rig has no H-alpha template of its own; with the library empty it says so instead of guessing.
    fireEvent.click(screen.getByRole('checkbox', { name: /C925 data/ }));
    expect(screen.getByText('No H-alpha template in this database or the library')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    expect(await screen.findByText('Saved plan revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0].revision).toBe(0);
    expect(saves[0].objectives).toHaveLength(1);
    expect(saves[0].objectives[0]).toMatchObject({ bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours', value: 6 }, priority: 1 });
    expect(saves[0].contributions).toHaveLength(1);
    expect(saves[0].contributions[0]).toMatchObject({ rig_id: redcat.rig.id, exposure_seconds: 600, template: { template_id: 2, filter_name: 'H-alpha' }, enabled: true, panel_ids: [] });
    // Switching the objective's bandpass drops templates chosen for the old one.
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'red' } });
    expect(screen.getByLabelText('C925 data template for Red')).toHaveValue('');
    expect(screen.queryByTestId('frames-redcat-h_alpha')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].revision).toBe(1);
    expect(saves[1].contributions).toHaveLength(0);
  });

  it('lets each rig own panels of a mosaic and names the panels nobody covers', async () => {
    const objective = { id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 6 }, priority: 1 };
    const stored: DirectorPlanDraft = { project_id: 'project', revision: 2, objectives: [objective], updated_at_ms: 1, contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: null, template_id: 1, name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 300, panel_ids: [], enabled: true },
    ] };
    const { saves } = fixture(stored, { rows: 2, columns: 1, overlap_percent: 20 }); mount();
    await screen.findByText(/3 templates/);
    expect(await screen.findByTestId('plan-coverage')).toHaveTextContent('Every objective has a rig on all 2 panels.');
    expect(screen.getByRole('checkbox', { name: 'RedCat 61 shoots every panel' })).toBeChecked();
    // Two panels at 72 frames each is 144 frames, twelve hours.
    expect(screen.getByText('144 frames, 12 h')).toBeInTheDocument();
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('72 per panel');
    fireEvent.click(screen.getByRole('checkbox', { name: 'RedCat 61 shoots panel r1c1' }));
    expect(screen.getByRole('checkbox', { name: 'RedCat 61 shoots every panel' })).not.toBeChecked();
    expect(screen.getByTestId('plan-coverage')).toHaveTextContent('H-alpha: no rig on panel r1c1.');
    expect(screen.getByText('72 frames, 6.0 h')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].contributions[0].panel_ids).toEqual(['r2c1']);
    // Ticking the panel back is the same as owning them all: no list.
    fireEvent.click(screen.getByRole('checkbox', { name: 'RedCat 61 shoots panel r1c1' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].contributions[0].panel_ids).toEqual([]);
  });

  it('shows a saved plan read only and counts frames goals per rig', async () => {
    const objective = { id: 'o1', bandpass_id: 'luminance', purpose: 'unsaturated_stars', goal: { kind: 'frames' as const, value: 40 }, priority: 2 };
    const stored: DirectorPlanDraft = { project_id: 'project', revision: 4, objectives: [objective], updated_at_ms: 1, contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: null, template_id: 3, name: 'Lum', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 90, panel_ids: [], enabled: true },
    ] };
    fixture(stored); mount(false);
    expect(await screen.findByLabelText('Objective goal')).toHaveValue(40);
    expect(screen.getByLabelText('Objective goal unit')).toHaveValue('frames');
    expect(await screen.findByTestId('frames-redcat-luminance')).toHaveTextContent('40');
    expect(screen.getByText('40 frames, 1.0 h')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save plan' })).not.toBeInTheDocument();
    expect(screen.getByText('Read only')).toBeInTheDocument();
    expect(framesFor({ kind: 'hours', value: 1 }, 7)).toBe(515);
    expect(rigTotals(stored)[0]).toEqual({ rigId: redcat.rig.id, frames: 40, hours: 1 });
  });
});
