import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import FramingView from '../director/FramingView';
import type { DirectorFramingDraft, DirectorMosaicPreview } from '../../api/directorTypes';
import type { SkyPreview } from '../../api/types';
import { moveBy, offsetFrom, skyAtStage, stackMatrix, thumbnailFov, toStage } from '../director/framingModel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rigA = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61',
  profile: null, field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const rigB = { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925', revision: 1 }, catalog_slug: 'c925', catalog_name: 'C925 data', profile: null, field_of_view: null, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const seed = { name: 'M31', center: { ra_degrees: 10.6847, dec_degrees: 41.269 }, position_angle_degrees: 35 };

/** One panel with a solved H-alpha stack one arcsecond per pixel, north up and east left, centred on the seed; one without. */
const solvedStack: SkyPreview = { url: '/api/db/redcat/stack-previews/job/0/preview?v=1', width: 200, height: 100, kind: 'mono', filter: 'Ha',
  wcs: { crpix1: 99.5, crpix2: 49.5, crval1: seed.center.ra_degrees, crval2: seed.center.dec_degrees, cd11: -1 / 3600, cd12: 0, cd21: 0, cd22: 1 / 3600 } };
const mosaic: DirectorMosaicPreview = { project: { id: 'project', name: 'Andromeda', revision: 1 }, activation_revision: 1, framing_revision: 1, framing_stale: false, warnings: [], panels: [
  { panel_id: 'r1c1', rig: rigA.rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', target_guid: 'g1', target_id: 7, target_name: 'M31 r1c1', progress: { desired: 72, acquired: 40, accepted: 36 }, status: 'ready', preview: solvedStack },
  { panel_id: 'r2c1', rig: rigA.rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', target_guid: 'g2', target_id: 8, target_name: 'M31 r2c1', progress: { desired: 72, acquired: 0, accepted: 0 }, status: 'no_stack', preview: null },
] };

function fixture(existing: DirectorFramingDraft | null = null) {
  const saves: DirectorFramingDraft[] = [];
  const cutouts: string[] = [];
  let draft = existing;
  server.use(
    http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft }))),
    http.put('/api/director/v1/projects/project/framing', async ({ request }) => {
      const body = await request.json() as DirectorFramingDraft;
      saves.push(body);
      draft = { ...body, revision: body.revision + 1, updated_at_ms: 5 };
      return HttpResponse.json(ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft }));
    }),
    http.get('/api/director/v1/sky/surveys', () => HttpResponse.json(ok([
      { id: 'dss2_color', name: 'DSS2 color', hips: 'CDS/P/DSS2/color', kind: 'broadband', bandpass: 'Plates', attribution: 'DSS2 via CDS' },
      { id: 'finkbeiner_halpha', name: 'Finkbeiner H-alpha composite', hips: 'CDS/P/Finkbeiner', kind: 'narrowband', bandpass: 'H-alpha', attribution: 'Finkbeiner via CDS' },
    ]))),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB]))),
    http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok(mosaic))),
    http.post('/api/director/v1/projects/project/feasibility', () => HttpResponse.json(ok({ center: seed.center, target_name: 'M31', nights: 7, rigs: [], warnings: ['No rig has a site yet, so nothing can be timed.'] }))),
    http.get('/api/director/v1/sky/cutout', ({ request }) => {
      cutouts.push(new URL(request.url).search);
      if (cutouts.length === 1) return HttpResponse.json(ok({ state: 'generating' }), { status: 202 });
      return HttpResponse.arrayBuffer(new Uint8Array([255, 216, 255]).buffer, { status: 200, headers: { 'content-type': 'image/jpeg' } });
    }),
    http.get('/api/director/v1/sky/resolve', ({ request }) => {
      const name = new URL(request.url).searchParams.get('name');
      return name === 'NGC 7000'
        ? HttpResponse.json(ok({ query: name, name: 'NGC 7000', ra_degrees: 314.75, dec_degrees: 44.37, source: 'Sesame' }))
        : HttpResponse.json({ success: false, data: null, error: `No catalog knows "${name}"` }, { status: 404 });
    }),
  );
  return { saves, cutouts };
}
function mount(canWrite = true, withSeed = true, preferredRigIds: string[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<FramingView projectId="project" seed={withSeed ? seed : null} preferredRigIds={preferredRigIds} />, { wrapper: Wrapper });
}
/** Point the stage at a stage-pixel position; the stage is laid out at its natural width. */
function pointer(x: number, y: number) {
  return { button: 0, pointerId: 1, clientX: x, clientY: y };
}

describe('Framing view', () => {
  beforeEach(() => {
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:stage'), revokeObjectURL: vi.fn() }));
  });
  afterEach(() => vi.unstubAllGlobals());

  it('seeds from the catalog target, frames with a rig, polls the survey, and saves a first draft', async () => {
    const { saves, cutouts } = fixture(); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35);
    // The first rig with optics frames on its own once rigs load; choosing it
    // by hand before or after that draws the same one rectangle, at once.
    fireEvent.change(screen.getByLabelText('Panel rig'), { target: { value: rigA.rig.id } });
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel, 5.38° × 3.60° in all.');
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    // The rectangle is the rig's field, turned 35°: its corners are not axis-aligned.
    const points = document.querySelector('.framing-panel polygon')!.getAttribute('points')!.split(' ').map(pair => pair.split(',').map(Number));
    expect(points).toHaveLength(4);
    expect(new Set(points.map(([x]) => x.toFixed(0))).size).toBe(4);
    // The 202 was polled once and the image then took the stage. The poll
    // waits a second by design, so give a loaded test runner room.
    await waitFor(() => expect(cutouts.length).toBeGreaterThanOrEqual(2), { timeout: 8000 });
    await waitFor(() => expect(document.querySelector('.framing-stage img')).toHaveAttribute('src', 'blob:stage'), { timeout: 4000 });
    expect(cutouts[0]).toContain('survey=dss2_color');
    expect(cutouts[0]).toContain('width=1024');

    fireEvent.change(screen.getByLabelText('Mosaic rows'), { target: { value: '2' } });
    fireEvent.change(screen.getByLabelText('Panel overlap percent'), { target: { value: '15' } });
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('2 panels, 5.38° × 6.66° in all.');
    expect(document.querySelectorAll('.framing-panel text')).toHaveLength(2);
    fireEvent.click(screen.getByRole('checkbox', { name: /RedCat 61/ }));
    expect(document.querySelectorAll('.framing-overlay')).toHaveLength(1);
    expect(screen.getByRole('checkbox', { name: /C925 data/ })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    expect(await screen.findByText('Saved framing revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0]).toMatchObject({ project_id: 'project', revision: 0, target_name: 'M31', panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 },
      mosaic: { rows: 2, columns: 1, overlap_percent: 15 }, shown_rig_ids: [rigA.rig.id], survey_id: 'dss2_color' });
    // The view widened on its own to show the whole footprint.
    expect(saves[0].view_fov_degrees).toBeGreaterThan(5.38);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].revision).toBe(1);
  });

  it('starts from a saved draft, switches survey, and pans without moving the target', async () => {
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 3, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
      mosaic: { rows: 1, columns: 2, overlap_percent: 10 }, panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'finkbeiner_halpha', view_fov_degrees: 6, updated_at_ms: 1 };
    const { cutouts, saves } = fixture(stored); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('Heart');
    expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha');
    expect(screen.getByLabelText('Panel width degrees')).toHaveValue(2);
    await waitFor(() => expect(cutouts[0]).toContain('survey=finkbeiner_halpha'));
    expect(screen.getByText(/Finkbeiner H-alpha composite: H-alpha/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Survey'), { target: { value: 'dss2_color' } });
    await waitFor(() => expect(cutouts.some(c => c.includes('survey=dss2_color'))).toBeTruthy(), { timeout: 3000 });

    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    fireEvent.pointerDown(stage, { button: 0, clientX: 500, clientY: 400, pointerId: 1 });
    fireEvent.pointerMove(stage, { clientX: 400, clientY: 400, pointerId: 1 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2);
    await waitFor(() => expect(screen.getByText(/The view center is/)).not.toHaveTextContent('02h 32m 48.0s'));
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].revision).toBe(3);
    expect(saves[0].center).toEqual({ ra_degrees: 38.2, dec_degrees: 61.5 });
  });

  it('frames with the first rig that holds the project, moves the target by dragging the rectangle, and turns it by its handle', async () => {
    const { saves } = fixture(); mount(true, true, [rigB.rig.id, rigA.rig.id]);
    // rigB has no optics, so rigA frames by default; no click needed.
    await waitFor(() => expect(screen.getByLabelText('Panel rig')).toHaveValue(rigA.rig.id));
    expect(screen.getByTestId('framing-readout')).toHaveTextContent('M31');
    expect(screen.getByTestId('framing-readout')).toHaveTextContent('angle 35.0°');
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    // Inside the rectangle (the mock puts its corners at ±2.69° × ±1.8° around the center): drag 100 px east.
    fireEvent.pointerDown(stage, pointer(512, 384));
    fireEvent.pointerMove(stage, { pointerId: 1, clientX: 412, clientY: 384 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    const ra = Number((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value);
    expect(ra).toBeGreaterThan(seed.center.ra_degrees);
    expect(screen.getByText(/The view center is/)).toHaveTextContent('00h 42m 44.3s');
    // The handle sits past the top edge along the camera's up direction; dragging it due east of the center turns the camera to 90°.
    fireEvent.click(screen.getByRole('button', { name: 'Turn 90 degrees clockwise' }));
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(125);
    expect(screen.getByTestId('framing-rotate-handle')).toBeInTheDocument();
    const handle = document.querySelector('.framing-rotate circle')!;
    const hx = Number(handle.getAttribute('cx')); const hy = Number(handle.getAttribute('cy'));
    fireEvent.pointerDown(stage, pointer(hx, hy));
    fireEvent.pointerMove(stage, { pointerId: 1, clientX: 300, clientY: 384 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    await waitFor(() => expect(Number((screen.getByLabelText('Position angle degrees') as HTMLInputElement).value)).toBeCloseTo(90, 0));
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].panel_rig_id).toBe(rigA.rig.id);
    expect(saves[0].position_angle_degrees).toBeCloseTo(90, 0);
  });

  it('is read only without write access and explains a missing seed', async () => {
    fixture(); mount(false);
    await screen.findByLabelText('Target name');
    expect(screen.queryByRole('button', { name: 'Save framing' })).not.toBeInTheDocument();
    expect(screen.getByText('Read only')).toBeInTheDocument();
  });

  it('starts an unseeded project from a resolved name or typed coordinates', async () => {
    fixture();
    server.use(http.get('/api/director/v1/sky/resolve', ({ request }) => {
      const name = new URL(request.url).searchParams.get('name');
      return name === 'IC 1805'
        ? HttpResponse.json(ok({ query: name, name: 'IC 1805', ra_degrees: 38.2, dec_degrees: 61.45, source: 'CDS Sesame' }))
        : HttpResponse.json({ success: false, data: null, error: `No object named '${name}' in the catalogs` }, { status: 404 });
    }));
    mount(true, false);
    fireEvent.change(await screen.findByLabelText('Object name to resolve'), { target: { value: 'Nowhere' } });
    fireEvent.click(screen.getByRole('button', { name: 'Look up name' }));
    expect(await screen.findByRole('alert')).toHaveTextContent("No object named 'Nowhere'");
    fireEvent.change(screen.getByLabelText('Object name to resolve'), { target: { value: 'IC 1805' } });
    fireEvent.click(screen.getByRole('button', { name: 'Look up name' }));
    expect(await screen.findByLabelText('Target name')).toHaveValue('IC 1805');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2);
    fireEvent.change(screen.getByLabelText('Panel rig'), { target: { value: rigA.rig.id } });
    await waitFor(() => expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2));
    expect(screen.getByLabelText('Declination degrees')).toHaveValue(61.45);
  });

  it('starts from typed coordinates when no catalog knows the name', async () => {
    fixture(); mount(true, false);
    fireEvent.change(await screen.findByLabelText('Start RA degrees'), { target: { value: '400' } });
    fireEvent.change(screen.getByLabelText('Start Dec degrees'), { target: { value: '10' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use these coordinates' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('RA in degrees from 0 to 360');
    fireEvent.change(screen.getByLabelText('Start RA degrees'), { target: { value: '83.8' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use these coordinates' }));
    expect(await screen.findByLabelText('Target name')).toHaveValue('Target');
    expect(screen.getByLabelText('Declination degrees')).toHaveValue(10);
  });

  it('draws each solved stack where its plate solve puts it and lists every panel', async () => {
    fixture(); mount();
    await waitFor(() => expect(screen.getByTestId('framing-stacks')).toBeInTheDocument());
    expect(screen.getByText(/r1c1, RedCat 61: 36\/72 frames accepted; Ha stack placed by its solve\./)).toBeInTheDocument();
    expect(screen.getByText(/r2c1, RedCat 61: 0\/72 frames accepted; no stack yet\./)).toBeInTheDocument();
    const image = await screen.findByTestId('framing-stack');
    expect(image).toHaveAttribute('href', solvedStack.url);
    // The stack spans 200″ × 100″ around the view center, so its transform is a small rectangle there.
    const matrix = image.getAttribute('transform')!;
    expect(matrix).toMatch(/^matrix\(/);
    const [a, b, c, d, e, f] = matrix.slice(7, -1).split(' ').map(Number);
    expect(b).toBeCloseTo(0, 6); expect(c).toBeCloseTo(0, 6);
    // East (increasing RA, lower pixel x) is stage-left, so pixel x runs right on the stage; north (higher pixel y) is up, so pixel y runs up.
    expect(a).toBeGreaterThan(0); expect(d).toBeLessThan(0);
    expect(e + a * 100).toBeCloseTo(512, 0); expect(f + d * 50).toBeCloseTo(384, 0);
    fireEvent.click(screen.getByLabelText('Show finished stacks on the sky'));
    expect(screen.queryByTestId('framing-stack')).not.toBeInTheDocument();
  });

  it('finds a target by name and moves the framing and the view there', async () => {
    const { saves } = fixture(); mount();
    await screen.findByLabelText('Target name');
    fireEvent.change(screen.getByLabelText('Find a target'), { target: { value: 'Nowhere Nebula' } });
    fireEvent.click(screen.getByRole('button', { name: 'Go' }));
    expect(await screen.findByText(/No catalog knows/)).toBeInTheDocument();
    expect(screen.getByLabelText('Target name')).toHaveValue('M31');
    fireEvent.change(screen.getByLabelText('Find a target'), { target: { value: 'NGC 7000' } });
    fireEvent.keyDown(screen.getByLabelText('Find a target'), { key: 'Enter' });
    expect(await screen.findByText('Moved the target to NGC 7000.')).toBeInTheDocument();
    expect(screen.getByLabelText('Target name')).toHaveValue('NGC 7000');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(314.75);
    expect(screen.getByText(/The view center is/)).toHaveTextContent('20h 59m 00.0s');
    // Enter in the search box looked the name up; it did not save the draft.
    expect(saves).toHaveLength(0);
  });

  it('switches the survey from the chips on the sky and remembers it in the draft', async () => {
    const { cutouts, saves } = fixture(); mount();
    await waitFor(() => expect(cutouts.length).toBeGreaterThan(0));
    const chips = screen.getByRole('group', { name: 'Survey layers' });
    expect(chips).toHaveTextContent('DSS2');
    expect(chips).toHaveTextContent('Hα Finkbeiner');
    fireEvent.click(screen.getByRole('button', { name: 'Hα Finkbeiner' }));
    expect(screen.getByRole('button', { name: 'Hα Finkbeiner' })).toHaveAttribute('aria-pressed', 'true');
    await waitFor(() => expect(cutouts.some(query => query.includes('survey=finkbeiner_halpha'))).toBe(true));
    expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha');
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].survey_id).toBe('finkbeiner_halpha');
    expect(thumbnailFov({ extent: { width_degrees: 2, height_degrees: 1.5 }, panel: null })).toBeCloseTo(3.2, 5);
    expect(thumbnailFov({ extent: null, panel: null })).toBeCloseTo(1.6, 5);
  });

  it('projects sky positions onto the view plane the way the server does', () => {
    const view = { ra_degrees: 10, dec_degrees: 60 };
    expect(offsetFrom(view, view)).toEqual([0, 0]);
    const north = offsetFrom(view, { ra_degrees: 10, dec_degrees: 61 })!;
    expect(north[0]).toBeCloseTo(0, 9); expect(north[1]).toBeCloseTo(1, 3);
    // Two degrees of RA at 60° is one degree on the sky, toward the east.
    const east = offsetFrom(view, { ra_degrees: 12, dec_degrees: 60 })!;
    expect(east[0]).toBeCloseTo(1, 2); expect(Math.abs(east[1])).toBeLessThan(0.02);
    expect(offsetFrom(view, { ra_degrees: 190, dec_degrees: -60 })).toBeNull();
    expect(stackMatrix({ ...solvedStack, wcs: null }, view, 2)).toBeNull();
  });

  it('maps offsets to the stage with north up and east left', () => {
    expect(toStage([0, 0], 4)).toEqual([512, 384]);
    const [x, y] = toStage([1, 0.5], 4);
    expect(x).toBeLessThan(512);
    expect(y).toBeLessThan(384);
    const east = skyAtStage({ ra_degrees: 10, dec_degrees: 0 }, 4, 256, 384);
    expect(east.ra_degrees).toBeCloseTo(11, 5);
    expect(moveBy({ ra_degrees: 359.5, dec_degrees: 89.9 }, 1, 1).dec_degrees).toBe(90);
  });
});
