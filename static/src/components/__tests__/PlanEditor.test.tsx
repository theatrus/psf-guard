import { type MutableRefObject, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import PlanEditor, { type PlanRigControls, type RigChange } from '../director/PlanEditor';
import { DraftProvider } from '../director/pageDrafts';
import { usePageDrafts } from '../director/pageDraftsState';
import type { DirectorLibraryTemplate, DirectorPlanDraft } from '../../api/directorTypes';
import { convertGoal, framesFor, rigTotals } from '../director/planModel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const redcat = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile: null,
  field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const c925 = { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925', revision: 1 }, catalog_slug: 'c925', catalog_name: 'C925 data', profile: null, field_of_view: null, default_exposure_seconds: { broadband: 60, narrowband: 180 } };
const template = (id: number, name: string, filter: string, bandpass: string, kind: 'broadband' | 'narrowband', exposure: number) =>
  ({ id, guid: `00000000-0000-4000-8000-${String(id).padStart(12, '0')}`, profile_id: 'p', name, filter_name: filter, gain: 100, offset: 30, bin: 1, readout_mode: null, default_exposure: exposure, bandpass: { id: bandpass, name, kind } });

const libraryHa = { id: '11111111-1111-4111-8111-111111111111', revision: 1, name: 'Ha 600 shared', filter_name: 'Ha', gain: 200, offset: 50, bin: 2, readout_mode: null, default_exposure_seconds: 600, updated_at_ms: 1, bandpass: { id: 'h_alpha', name: 'H-alpha', kind: 'narrowband' as const } };

function fixture(existing: DirectorPlanDraft | null = null, mosaic: { rows: number; columns: number; overlap_percent: number } | null = null, library: DirectorLibraryTemplate[] = [libraryHa], rigFramings: Array<{ rig_id: string; center?: { ra_degrees: number; dec_degrees: number } | null; position_angle_degrees: number | null; mosaic: { rows: number; columns: number; overlap_percent: number }; panel: { width_degrees: number; height_degrees: number } | null }> = []) {
  const saves: DirectorPlanDraft[] = [];
  let plan = existing;
  server.use(
    http.get('/api/director/v1/templates', () => HttpResponse.json(ok(library))),
    http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, draft: mosaic ? {
      project_id: 'project', revision: 1, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.45 }, position_angle_degrees: 0, mosaic, panel_rig_id: null,
      panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 4, updated_at_ms: 1, rig_framings: rigFramings } : null }))),
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
type Drafts = ReturnType<typeof usePageDrafts>;
/** The editor on a page that keeps drafts, as the project workspace does. */
function OnPage({ out }: { out: MutableRefObject<Drafts | null> }) {
  const drafts = usePageDrafts();
  out.current = drafts;
  return <DraftProvider drafts={drafts}><PlanEditor projectId="project" /></DraftProvider>;
}
function mount(canWrite = true, extra: { drafts?: MutableRefObject<Drafts | null> } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(extra.drafts ? <OnPage out={extra.drafts} /> : <PlanEditor projectId="project" />, { wrapper: Wrapper });
}

describe('Plan editor', () => {
  it("says which of a rig's templates the server left out, and why", async () => {
    fixture();
    server.use(http.get('/api/director/v1/catalogs/redcat/templates', () => HttpResponse.json(ok({ catalog_slug: 'redcat', catalog_name: 'RedCat 61', rig: redcat.rig,
      templates: [template(1, 'Ha 300', 'Ha', 'h_alpha', 'narrowband', 300)],
      warnings: ['Template #5 Ha deep: its Moon avoidance rules are out of range.'] }))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}><PlanEditor projectId="project" linkedRigIds={[redcat.rig.id]} /></QueryClientProvider>);
    expect(await screen.findByRole('note', { name: 'RedCat 61 templates left out' })).toHaveTextContent('Template #5 Ha deep: its Moon avoidance rules are out of range.');
  });

  it('shows each rig as one block with its project, and folds rigs outside the plan away', async () => {
    fixture();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}>
      <PlanEditor
        projectId="project"
        linkedRigIds={[c925.rig.id]}
        rigExtras={rig => rig.rig.id === c925.rig.id
          ? { place: 'project “Heart subs”', below: <button type="button">Edit in Target Scheduler</button> }
          : {}}
        footer={<p>Attach area</p>}
      />
    </QueryClientProvider>);
    // Wait for the loaded plan: until then only the linked rigs show.
    await screen.findByText('Other rigs (1)');
    const rigs = screen.getByRole('region', { name: 'Rigs' });
    const linked = within(rigs).getByRole('group', { name: 'C925 data' });
    expect(linked).toHaveTextContent('project “Heart subs”');
    expect(within(linked).getByRole('button', { name: 'Edit in Target Scheduler' })).toBeInTheDocument();
    // RedCat has no project here and takes no part: it waits under the fold.
    const others = screen.getByText('Other rigs (1)').closest('details')!;
    expect(others).not.toHaveAttribute('open');
    expect(within(others).getByRole('group', { name: 'RedCat 61' })).toBeInTheDocument();
    expect(within(rigs).getByText('Attach area')).toBeInTheDocument();
  });

  it('gives a rig framed on its own its own panels, and leaves it out of the shared coverage check', async () => {
    const objective = { id: 'o1', bandpass_id: 'red', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 2 }, priority: 1 };
    const plan = { project_id: 'project', revision: 1, updated_at_ms: 1, objectives: [objective], contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: c925.rig.id, template: { template_guid: null, template_id: 7, name: 'Red', filter_name: 'R', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 120, panel_ids: [], enabled: true },
    ] };
    fixture(plan, { rows: 2, columns: 1, overlap_percent: 20 }, [], [{ rig_id: c925.rig.id, center: null, position_angle_degrees: 90, mosaic: { rows: 1, columns: 3, overlap_percent: 10 }, panel: null }]);
    mount();
    await screen.findByText(/1 template/);
    // C925 owns its own three panels; RedCat would own the shared two.
    const own = screen.getByRole('group', { name: 'C925 data panels' });
    expect(own).toHaveTextContent('Separate panels:');
    expect(within(own).getByLabelText('C925 data shoots panel r1c3')).toBeChecked();
    expect(within(own).queryByLabelText('C925 data shoots panel r2c1')).not.toBeInTheDocument();
    // Three panels at 60 frames each.
    expect(screen.getByText('180 frames, 6.0 h')).toBeInTheDocument();
    // The shared grid has nobody on it for red, but the rig framed on its own is not a gap in it.
    expect(screen.getByTestId('plan-coverage')).toHaveTextContent('Red: no rig on any panel');
  });

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

  it("offers a library template the rig's database already holds once, as the database's, and binds the plan to it", async () => {
    // Copied in from RedCat's own Ha 300: the same settings under a new id.
    const copied = { ...libraryHa, id: '22222222-2222-4222-8222-222222222222', name: 'Ha 300 copy', gain: 100, offset: 30, bin: 1, default_exposure_seconds: 300 };
    // Written into RedCat by an earlier activation under the library's id;
    // the row's gain was changed there since.
    const written = { ...libraryHa, id: '00000000-0000-4000-8000-000000000003', name: 'Lum shared', filter_name: 'L', gain: 0, offset: 10, bin: 1, default_exposure_seconds: 60, bandpass: { id: 'luminance', name: 'Luminance', kind: 'broadband' as const } };
    const objectives = [
      { id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 6 }, priority: 1 },
      { id: 'o2', bandpass_id: 'luminance', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 2 }, priority: 1 },
    ];
    const stored: DirectorPlanDraft = { project_id: 'project', revision: 2, objectives, updated_at_ms: 1, contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: copied.id, template_id: null, name: copied.name, filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 240, panel_ids: [], enabled: true },
      { id: 'c2', objective_id: 'o2', rig_id: redcat.rig.id, template: { template_guid: written.id, template_id: null, name: written.name, filter_name: 'L', gain: 0, offset: 10, bin: 1, readout_mode: null }, exposure_seconds: 60, panel_ids: [], enabled: true },
    ] };
    const { saves } = fixture(stored, null, [libraryHa, copied, written]);
    const drafts: MutableRefObject<Drafts | null> = { current: null };
    mount(true, { drafts });
    const ha = await screen.findByLabelText('RedCat 61 template for H-alpha');
    await waitFor(() => expect(ha).toHaveValue('db:1'));
    expect(within(ha).queryByRole('option', { name: /Ha 300 copy/ })).not.toBeInTheDocument();
    // A library template with other settings is still on offer.
    expect(within(ha).getByRole('option', { name: /Ha 600 shared/ })).toBeInTheDocument();
    const lum = screen.getByLabelText('RedCat 61 template for Luminance');
    expect(lum).toHaveValue('db:3');
    expect(within(lum).queryByRole('group', { name: 'Library, written on activation' })).not.toBeInTheDocument();
    // The rig keeps its exposure, and the binding alone is nothing to save.
    expect(screen.getByLabelText('RedCat 61 exposure for H-alpha')).toHaveValue(240);
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['plan']));
    expect(drafts.current!.unsaved).toHaveLength(0);
    // The next save names the database's rows.
    fireEvent.change(screen.getByLabelText('RedCat 61 exposure for H-alpha'), { target: { value: '300' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toBeNull();
    const sent = Object.fromEntries(saves[0].contributions.map(c => [c.id, c]));
    expect(sent.c2.template.template_id).toBe(3);
    expect(sent.c1).toMatchObject({ exposure_seconds: 300, template: { template_guid: '00000000-0000-4000-8000-000000000001', name: 'Ha 300' } });
  });

  it("shows each part's progress and status, and the rig's in a summary row on top", async () => {
    const objectives = [
      { id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 2 }, priority: 1 },
      { id: 'o2', bandpass_id: 'luminance', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 1 }, priority: 1 },
    ];
    const part = (id: string, objective: string, template: number, name: string, filter: string, enabled = true) =>
      ({ id, objective_id: objective, rig_id: redcat.rig.id, template: { template_guid: null, template_id: template, name, filter_name: filter, gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 300, panel_ids: [], enabled });
    fixture({ project_id: 'project', revision: 1, updated_at_ms: 1, objectives, contributions: [part('c1', 'o1', 1, 'Ha 300', 'Ha'), part('c2', 'o2', 3, 'Lum', 'L')] }, null, []);
    const frames = (desired: number, acquired: number, accepted: number, rejected: number) => ({ desired, acquired, accepted, rejected });
    server.use(http.get('/api/director/v1/projects/project/plan/progress', () => HttpResponse.json(ok({ rigs: [{
      rig_id: redcat.rig.id, project: { name: 'Heart Nebula', state: 1 }, note: null, other: frames(0, 0, 0, 0), total: frames(36, 30, 26, 2),
      objectives: [{ objective_id: 'o1', frames: frames(24, 26, 24, 2), exposure_plans: 2 }, { objective_id: 'o2', frames: frames(12, 4, 2, 0), exposure_plans: 1 }],
    }] }))));
    mount();
    const summary = await screen.findByTestId('summary-redcat');
    await waitFor(() => expect(summary).toHaveTextContent('All objectives · Heart Nebula'));
    expect(summary).toHaveTextContent('36 frames, 3.0 h');
    expect(summary).toHaveTextContent('26 / 36');
    expect(summary).toHaveTextContent('Active');
    const rows = within(screen.getByRole('group', { name: 'RedCat 61' })).getAllByRole('row');
    // Header, summary, then one row per objective.
    expect(rows[2]).toHaveTextContent('24 / 24');
    expect(rows[2]).toHaveTextContent('26 taken, 2 rejected');
    expect(rows[2]).toHaveTextContent('Done');
    expect(rows[3]).toHaveTextContent('2 / 12');
    expect(rows[3]).toHaveTextContent('Active');
    fireEvent.click(screen.getByRole('checkbox', { name: 'RedCat 61 shoots Luminance' }));
    expect(rows[3]).toHaveTextContent('Off');
  });

  it("takes a picked template's exposure, after goals and a hand-set exposure", async () => {
    const objective = { id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 6 }, priority: 1 };
    fixture({ project_id: 'project', revision: 1, updated_at_ms: 1, objectives: [objective], contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: null, template_id: 1, name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null },
        exposure_seconds: 120, panel_ids: [], enabled: true, goal: { kind: 'hours', value: 10 } },
    ] });
    mount();
    const exposure = await screen.findByLabelText('RedCat 61 exposure for H-alpha');
    expect(exposure).toHaveValue(120);
    // The library's Ha 600 brings 600 s: 10 h is 60 frames.
    fireEvent.change(screen.getByLabelText('RedCat 61 template for H-alpha'), { target: { value: 'lib:11111111-1111-4111-8111-111111111111' } });
    expect(screen.getByLabelText('RedCat 61 exposure for H-alpha')).toHaveValue(600);
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('60');
    // Back to the rig's own Ha 300; the rig's goal stays.
    fireEvent.change(screen.getByLabelText('RedCat 61 template for H-alpha'), { target: { value: 'db:1' } });
    expect(screen.getByLabelText('RedCat 61 exposure for H-alpha')).toHaveValue(300);
    expect(screen.getByLabelText('RedCat 61 goal for H-alpha')).toHaveValue(10);
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('120');
  });

  it('lets the Rigs tab add and drop rigs, listing only the rigs that shoot the plan', async () => {
    fixture(null, null, []);
    const controls: MutableRefObject<PlanRigControls | null> = { current: null };
    const reported: Array<{ rigIds: string[]; objectives: number; ready: boolean }> = [];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}><PlanEditor projectId="project" controls={controls} onRigsChange={state => reported.push(state)} /></QueryClientProvider>);
    expect(await screen.findByText('No rigs yet')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Add objective' }));
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'h_alpha' } });
    await waitFor(() => expect(reported.at(-1)).toEqual({ rigIds: [], objectives: 1, ready: true }));
    await waitFor(() => expect(controls.current).not.toBeNull());
    // Rigs join from elsewhere, with a template for each objective as a tick gives them.
    act(() => controls.current!.setRig(redcat.rig.id, true));
    const rig = await screen.findByRole('group', { name: 'RedCat 61' });
    expect(within(rig).getByLabelText('RedCat 61 template for H-alpha')).toHaveValue('db:1');
    expect(within(rig).queryByRole('checkbox', { name: /takes part/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'C925 data' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Other rigs/)).not.toBeInTheDocument();
    await waitFor(() => expect(reported.at(-1)).toEqual({ rigIds: [redcat.rig.id], objectives: 1, ready: true }));
    act(() => controls.current!.setRig(redcat.rig.id, false));
    await waitFor(() => expect(screen.queryByRole('group', { name: 'RedCat 61' })).not.toBeInTheDocument());
    expect(reported.at(-1)).toEqual({ rigIds: [], objectives: 1, ready: true });
  });
  it('lets a slow rig carry its own goal, with an f-ratio suggestion, and saves it', async () => {
    const slow = { ...redcat, rig: { ...redcat.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'C925 slow' }, catalog_slug: 'redcat', catalog_name: 'C925 slow',
      field_of_view: { width_degrees: 0.57, height_degrees: 0.38, pixel_scale_arcsec: 0.33, focal_ratio: 10 } };
    const { saves } = fixture(null, null, []);
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([slow]))));
    mount();
    await screen.findByText(/3 templates/);
    fireEvent.click(screen.getByRole('button', { name: 'Add objective' }));
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'h_alpha' } });
    fireEvent.change(screen.getByLabelText('Objective goal'), { target: { value: '6' } });
    fireEvent.click(screen.getByRole('checkbox', { name: /C925 slow/ }));
    // The plan's 6 h, and an f/10 suggestion: four times f/5's time.
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('72');
    fireEvent.click(screen.getByRole('button', { name: 'f/10.0: 24 h' }));
    expect(screen.getByLabelText('C925 slow goal for H-alpha')).toHaveValue(24);
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('288');
    expect(screen.getByText('288 frames, 24 h')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].objectives[0].goal).toEqual({ kind: 'hours', value: 6 });
    expect(saves[0].contributions[0].goal).toEqual({ kind: 'hours', value: 24 });
    // Back to the plan's goal.
    fireEvent.click(screen.getByRole('button', { name: "C925 slow uses the plan's goal for H-alpha" }));
    expect(screen.getByTestId('frames-redcat-h_alpha')).toHaveTextContent('72');
  });

  it('adds an objective in hours, binds a rig through its matching template, and saves frames per rig', async () => {
    const { saves } = fixture(null, null, []); mount();
    expect(await screen.findByText('No objectives yet')).toBeInTheDocument();
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
    // The other rig has no H-alpha template of its own; with the library
    // empty it stays out and says so instead of joining with nothing.
    fireEvent.click(screen.getByRole('checkbox', { name: /C925 data/ }));
    expect(screen.getByRole('status')).toHaveTextContent('C925 data: no H-alpha template');
    expect(screen.getByRole('checkbox', { name: /C925 data/ })).not.toBeChecked();
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }));
    expect(await screen.findByText('Saved plan revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0].revision).toBe(0);
    expect(saves[0].objectives).toHaveLength(1);
    expect(screen.queryByLabelText('Objective priority')).not.toBeInTheDocument();
    expect(saves[0].objectives[0]).toMatchObject({ bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours', value: 6 }, priority: 1 });
    expect(saves[0].contributions).toHaveLength(1);
    expect(saves[0].contributions[0]).toMatchObject({ rig_id: redcat.rig.id, exposure_seconds: 600, template: { template_id: 2, filter_name: 'H-alpha' }, enabled: true, panel_ids: [] });
    // Switching the objective's bandpass drops templates chosen for the old one.
    fireEvent.change(screen.getByLabelText('Objective bandpass'), { target: { value: 'red' } });
    expect(screen.getByText('No Red template in this database or the library')).toBeInTheDocument();
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
  it('hands its edits to the page save bar and leaves out its own Save button', async () => {
    // Activation reads the saved plan: a raised goal left unsaved was what
    // it lost. On the project page the plan saves through the page's bar.
    const objective = { id: 'o1', bandpass_id: 'luminance', purpose: 'unsaturated_stars', goal: { kind: 'frames' as const, value: 40 }, priority: 2 };
    const stored: DirectorPlanDraft = { project_id: 'project', revision: 4, objectives: [objective], updated_at_ms: 1, contributions: [
      { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: null, template_id: 3, name: 'Lum', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 90, panel_ids: [], enabled: true },
    ] };
    const { saves } = fixture(stored);
    const drafts: MutableRefObject<Drafts | null> = { current: null };
    mount(true, { drafts });
    const goal = await screen.findByLabelText('Objective goal');
    expect(goal).toHaveValue(40);
    expect(screen.queryByRole('button', { name: 'Save plan' })).not.toBeInTheDocument();
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['plan']));
    expect(drafts.current!.unsaved).toHaveLength(0);

    // Typed over: emptied, then a new number, not a 0 in the way.
    fireEvent.change(goal, { target: { value: '' } });
    expect(goal).toHaveValue(null);
    fireEvent.change(goal, { target: { value: '120' } });
    expect(goal).toHaveValue(120);
    await waitFor(() => expect(drafts.current!.unsaved.map(section => section.label)).toEqual(['Exposures']));

    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves).toHaveLength(1);
    expect(saves[0].objectives[0].goal).toEqual({ kind: 'frames', value: 120 });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(0));

    // Discard puts the saved plan back.
    fireEvent.change(goal, { target: { value: '7' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    act(() => drafts.current!.discardAll());
    await waitFor(() => expect(goal).toHaveValue(120));
  });
});

/** The editor with the Rigs tab's controls on a page that keeps drafts, as
 *  the workspace has it, and the query client the test can refresh. */
function mountWorkspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const controls: MutableRefObject<PlanRigControls | null> = { current: null };
  const drafts: MutableRefObject<Drafts | null> = { current: null };
  function Page() {
    const page = usePageDrafts();
    drafts.current = page;
    return <DraftProvider drafts={page}><PlanEditor projectId="project" controls={controls} /></DraftProvider>;
  }
  render(<QueryClientProvider client={client}><Page /></QueryClientProvider>);
  return { client, controls, drafts };
}

describe('Plan editor on the workspace', () => {
  const objective = { id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours' as const, value: 6 }, priority: 1 };
  const stored = (revision = 4, value = 6): DirectorPlanDraft => ({ project_id: 'project', revision, updated_at_ms: 1, objectives: [{ ...objective, goal: { kind: 'hours', value } }], contributions: [
    { id: 'c1', objective_id: 'o1', rig_id: redcat.rig.id, template: { template_guid: null, template_id: 1, name: 'Ha 300', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: null }, exposure_seconds: 300, panel_ids: [], enabled: true },
  ] });

  it('drops a rig by switching its parts off, and takes the same parts back without doubling them', async () => {
    const { saves } = fixture(stored());
    const { controls, drafts } = mountWorkspace();
    await screen.findByText(/3 templates/);
    await waitFor(() => expect(controls.current).not.toBeNull());
    let change: RigChange | undefined;
    act(() => { change = controls.current!.setRig(redcat.rig.id, false); });
    expect(change).toEqual({ done: true, note: null });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(drafts.current!.unsaved[0].changes).toEqual(['RedCat 61 dropped']);
    // Discard, then on again: the rig had its part back already.
    act(() => drafts.current!.discardAll());
    act(() => { change = controls.current!.setRig(redcat.rig.id, true); });
    expect(change).toEqual({ done: true, note: null });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(0));
    // Off and saved: the part stays, switched off, for activation to see.
    act(() => { controls.current!.setRig(redcat.rig.id, false); });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[0].contributions).toEqual([expect.objectContaining({ id: 'c1', enabled: false })]);
    // On again: the same part, not a second one.
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(0));
    act(() => { controls.current!.setRig(redcat.rig.id, true); });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[1].contributions).toEqual([expect.objectContaining({ id: 'c1', enabled: true })]);
  });

  it('says a rig with no template for the band does not join', async () => {
    fixture(stored(), null, []);
    const { controls } = mountWorkspace();
    await screen.findByText(/3 templates/);
    await waitFor(() => expect(controls.current).not.toBeNull());
    let change: RigChange | undefined;
    act(() => { change = controls.current!.setRig(c925.rig.id, true); });
    expect(change).toEqual({ done: false, note: 'no H-alpha template' });
    expect(screen.queryByRole('group', { name: 'C925 data' })).not.toBeInTheDocument();
  });

  it('loads a plan saved with a rig on one objective twice as one part', async () => {
    const twice = stored();
    twice.contributions.push({ ...twice.contributions[0], id: 'c2', exposure_seconds: 600 });
    const { saves } = fixture(twice);
    const { drafts } = mountWorkspace();
    const goal = await screen.findByLabelText('Objective goal');
    expect(screen.getAllByLabelText('RedCat 61 exposure for H-alpha')).toHaveLength(1);
    fireEvent.change(goal, { target: { value: '8' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[0].contributions.map(c => c.id)).toEqual(['c1']);
  });

  it('keeps edits typed while a save runs', async () => {
    const { saves } = fixture(stored());
    let release = () => {};
    const gate = new Promise<void>(resolve => { release = resolve; });
    server.use(http.put('/api/director/v1/projects/project/plan', async ({ request }) => {
      const body = await request.json() as DirectorPlanDraft;
      saves.push(body);
      if (saves.length === 1) await gate;
      return HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, plan: { ...body, revision: body.revision + 1, updated_at_ms: 9 } }));
    }));
    const { drafts } = mountWorkspace();
    const goal = await screen.findByLabelText('Objective goal');
    fireEvent.change(goal, { target: { value: '8' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    let saving: Promise<unknown> = Promise.resolve();
    act(() => { saving = drafts.current!.saveAll(); });
    await waitFor(() => expect(saves).toHaveLength(1));
    fireEvent.change(goal, { target: { value: '9' } });
    await act(async () => { release(); await saving; });
    expect(saves[0].objectives[0].goal).toEqual({ kind: 'hours', value: 8 });
    // The 9 stays, unsaved, over the new revision, and saves against it.
    expect(screen.getByLabelText('Objective goal')).toHaveValue(9);
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[1]).toMatchObject({ revision: 5, objectives: [expect.objectContaining({ goal: { kind: 'hours', value: 9 } })] });
  });

  it('keeps unsaved edits over a newer saved copy and marks the conflict', async () => {
    fixture(stored());
    const { client, drafts } = mountWorkspace();
    const goal = await screen.findByLabelText('Objective goal');
    fireEvent.change(goal, { target: { value: '8' } });
    // Another browser saves, or an attach brings a plan in; the page refetches.
    server.use(http.get('/api/director/v1/projects/project/plan', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, plan: stored(5, 12) }))));
    await act(() => client.invalidateQueries({ queryKey: ['directorPlan', 'project'] }));
    expect(await screen.findByText(/This plan changed since you loaded it/)).toBeInTheDocument();
    expect(screen.getByLabelText('Objective goal')).toHaveValue(8);
    expect(await drafts.current!.saveAll()).toEqual({ label: 'Exposures', reason: 'the plan changed elsewhere; reload it first' });
    // Reload asks, then shows the saved copy.
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    expect(confirm).toHaveBeenCalled();
    await waitFor(() => expect(screen.getByLabelText('Objective goal')).toHaveValue(12));
    expect(screen.queryByText(/This plan changed since you loaded it/)).not.toBeInTheDocument();
    confirm.mockRestore();
  });
});
