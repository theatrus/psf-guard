import { type MutableRefObject, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import FramingView from '../director/FramingView';
import { DraftProvider } from '../director/pageDrafts';
import { usePageDrafts } from '../director/pageDraftsState';
import type { DirectorFramingDraft, DirectorMosaicPreview } from '../../api/directorTypes';
import type { SkyPreview } from '../../api/types';
import { angleAt, moveBy, offsetFrom, skyAtStage, stackMatrix, thumbnailFov, toStage, viewAt } from '../director/framingModel';
import { gridLayout } from '../director/mosaicLayout';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rigA = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61',
  profile: null, field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const rigB = { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925', revision: 1 }, catalog_slug: 'c925', catalog_name: 'C925 data', profile: null, field_of_view: null, default_exposure_seconds: { broadband: 120, narrowband: 300 } };
const seed = { name: 'M31', center: { ra_degrees: 10.6847, dec_degrees: 41.269 }, position_angle_degrees: 35 };

/** One panel with a solved H-alpha stack one arcsecond per pixel, north up and east left, centred on the seed; one without. */
const solvedStack: SkyPreview = { url: '/api/db/redcat/stack-previews/job/0/preview?v=1', width: 200, height: 100, kind: 'mono', filter: 'Ha', key: 'mono:Ha', label: 'Ha',
  wcs: { crpix1: 99.5, crpix2: 49.5, crval1: seed.center.ra_degrees, crval2: seed.center.dec_degrees, cd11: -1 / 3600, cd12: 0, cd21: 0, cd22: 1 / 3600 } };
/** The same panel's OIII stack, not yet solved. */
const oiiiStack: SkyPreview = { ...solvedStack, url: '/api/db/redcat/stack-previews/job/1/preview?v=1', filter: 'OIII', key: 'mono:OIII', label: 'OIII', wcs: null };
const mosaic: DirectorMosaicPreview = { project: { id: 'project', name: 'Andromeda', revision: 1 }, activation_revision: 1, framing_revision: 1, framing_stale: false, warnings: [], panels: [
  { panel_id: 'r1c1', rig: rigA.rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', target_guid: 'g1', target_id: 7, target_name: 'M31 r1c1', progress: { desired: 72, acquired: 40, accepted: 36 }, status: 'ready', preview: solvedStack, stacks: [solvedStack, oiiiStack] },
  { panel_id: 'r2c1', rig: rigA.rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', target_guid: 'g2', target_id: 8, target_name: 'M31 r2c1', progress: { desired: 72, acquired: 0, accepted: 0 }, status: 'no_stack', preview: null, stacks: [] },
] };
/** The same panels before anything was shot: none of them is locked by its frames. */
const unshot: DirectorMosaicPreview = { ...mosaic, panels: mosaic.panels.map(panel => ({ ...panel, progress: panel.progress && { ...panel.progress, acquired: 0, accepted: 0 } })) };

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
    http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok(unshot))),
    http.post('/api/director/v1/projects/project/feasibility', () => HttpResponse.json(ok({ center: seed.center, target_name: 'M31', nights: 7, rigs: [], warnings: ['No rig has a site yet, so nothing can be timed.'] }))),
    http.get('/api/director/v1/sky/cutout', ({ request }) => {
      cutouts.push(new URL(request.url).search);
      if (cutouts.length === 1) return HttpResponse.json(ok({ state: 'generating' }), { status: 202 });
      return HttpResponse.arrayBuffer(new Uint8Array([255, 216, 255]).buffer, { status: 200, headers: { 'content-type': 'image/jpeg' } });
    }),
    http.get('/api/director/v1/sky/search', ({ request }) => {
      const q = new URL(request.url).searchParams.get('q') ?? '';
      const items = q.toLowerCase().startsWith('nor') || q.toLowerCase().startsWith('ngc 70')
        ? [{ name: 'NGC 7000', common_name: 'North America Nebula', kind: 'nebula', ra_degrees: 314.75, dec_degrees: 44.37, matched: q.toLowerCase().startsWith('nor') ? 'North America Nebula' : 'NGC 7000', source: 'Seiza object catalog' },
           { name: 'NGC 7023', common_name: 'Iris Nebula', kind: 'nebula', ra_degrees: 315.4, dec_degrees: 68.16, matched: 'NGC 7023', source: 'Seiza object catalog' }]
        : [];
      return HttpResponse.json(ok({ query: q, local: { available: true, items }, online: null, online_state: 'skipped', online_cached: false }));
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
type Drafts = ReturnType<typeof usePageDrafts>;
/** The view on a page that keeps drafts, as the project workspace does. */
function mountOnPage(withSeed = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const out: MutableRefObject<Drafts | null> = { current: null };
  function Page() {
    const drafts = usePageDrafts();
    out.current = drafts;
    return <DraftProvider drafts={drafts}><FramingView projectId="project" seed={withSeed ? seed : null} /></DraftProvider>;
  }
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite: true }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  render(<Page />, { wrapper: Wrapper });
  return out;
}
/** Point the stage at a stage-pixel position; the stage is laid out at its natural width. */
function pointer(x: number, y: number) {
  return { button: 0, pointerId: 1, clientX: x, clientY: y };
}

describe('Framing view', () => {
  beforeEach(() => {
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:stage'), revokeObjectURL: vi.fn() }));
    // The drag mode and rotate-sky choices are remembered in the browser; each test starts from the defaults.
    window.localStorage.clear();
  });
  afterEach(() => vi.unstubAllGlobals());

  it('seeds from the catalog target, frames with a rig, polls the survey, and saves a first draft', async () => {
    const { saves, cutouts } = fixture(); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35);
    // The first rig with optics sizes the panels once rigs load, and says so.
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°'));
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 5.38° × 3.60°');
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    // The rectangle is the rig's field, turned 35°: its corners are not axis-aligned.
    const points = document.querySelector('.framing-panel polygon')!.getAttribute('points')!.split(' ').map(pair => pair.split(',').map(Number));
    expect(points).toHaveLength(4);
    expect(new Set(points.map(([x]) => x.toFixed(0))).size).toBe(4);
    // The 202 was polled once and the image then took the stage. The poll
    // waits a second by design, so give a loaded test runner room.
    await waitFor(() => expect(cutouts.length).toBeGreaterThanOrEqual(2), { timeout: 8000 });
    await waitFor(() => expect(document.querySelector('.framing-stage [data-testid="framing-sky"]')).toHaveAttribute('href', 'blob:stage'), { timeout: 4000 });
    // Rig data can widen the view after the initial sky request. Its current
    // tile is a little wider than that view, at the stage's pixel density.
    await waitFor(() => {
      const latest = cutouts.filter(query => query.includes('survey=dss2_color')).at(-1)!;
      expect(latest).toContain('width=2048');
      expect(latest).toContain('height=1536');
      const askedFov = Number(latest.match(/fov=([0-9.]+)/)![1]);
      const viewFov = Number((screen.getByLabelText('View width degrees') as HTMLInputElement).value);
      expect(askedFov).toBeCloseTo(viewFov * 1.3, 1);
    });

    fireEvent.change(screen.getByLabelText('Mosaic rows'), { target: { value: '2' } });
    fireEvent.change(screen.getByLabelText('Panel overlap percent'), { target: { value: '15' } });
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('2 panels · 5.38° × 6.66°');
    expect(document.querySelectorAll('.framing-panel text')).toHaveLength(2);
    fireEvent.click(screen.getByRole('checkbox', { name: /RedCat 61/ }));
    expect(document.querySelectorAll('.framing-overlay')).toHaveLength(1);
    // The outline names its rig, as the panels do.
    expect(document.querySelector('.framing-overlay-label')).toHaveTextContent('RedCat 61');
    // A rig without optics has no outline to show, and says why.
    expect(screen.queryByRole('checkbox', { name: /C925 data/ })).not.toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'C925 data' })).toHaveTextContent('No optics');
    expect(screen.getByRole('group', { name: 'RedCat 61' })).toHaveTextContent('Frames the plan · 2 × 1, 15%');

    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    expect(await screen.findByText('Saved framing revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0]).toMatchObject({ project_id: 'project', revision: 0, target_name: 'M31', panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 },
      layout: gridLayout({ rows: 2, columns: 1, overlap_percent: 15 }), shown_rig_ids: [rigA.rig.id], survey_id: 'dss2_color' });
    // The view widened on its own to show the whole footprint.
    expect(saves[0].view_fov_degrees).toBeGreaterThan(5.38);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1].revision).toBe(1);
  });

  it('starts from a saved draft, switches survey, and pans without moving the target', async () => {
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 3, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
      layout: gridLayout({ rows: 1, columns: 2, overlap_percent: 10 }), panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'finkbeiner_halpha', view_fov_degrees: 6, updated_at_ms: 1 };
    const { cutouts, saves } = fixture(stored); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('Heart');
    expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha');
    expect(screen.getByLabelText('Panel width degrees')).toHaveValue(2);
    await waitFor(() => expect(cutouts.some(query => query.includes('survey=finkbeiner_halpha'))).toBe(true));
    expect(screen.getByText(/Finkbeiner H-alpha composite: H-alpha/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Survey'), { target: { value: 'dss2_color' } });
    await waitFor(() => expect(cutouts.some(c => c.includes('survey=dss2_color'))).toBeTruthy(), { timeout: 3000 });

    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    // While the pointer moves, the loaded tile slides under the view at once:
    // no new request, just a new matrix on the image already on screen.
    await waitFor(() => expect(screen.getByTestId('framing-sky')).toBeInTheDocument());
    const before = cutouts.length;
    const resting = screen.getByTestId('framing-sky').getAttribute('transform')!;
    expect(resting).toMatch(/^matrix\(/);
    fireEvent.pointerDown(stage, { button: 0, clientX: 500, clientY: 400, pointerId: 1 });
    fireEvent.pointerMove(stage, { buttons: 1, clientX: 400, clientY: 400, pointerId: 1 });
    const moved = screen.getByTestId('framing-sky').getAttribute('transform')!;
    expect(moved).toMatch(/^matrix\(/);
    expect(moved).not.toBe(resting);
    // The tile spans 1.3 views at 2048 px over a 1024-unit stage: 0.65 stage units per tile pixel.
    expect(Number(moved.slice(7, -1).split(' ')[0])).toBeCloseTo(0.65, 2);
    expect(cutouts.length).toBe(before);
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2);
    await waitFor(() => expect(screen.getByTestId('framing-view-center')).not.toHaveTextContent('02h 32m 48.0s'));
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].revision).toBe(3);
    expect(saves[0].center).toEqual({ ra_degrees: 38.2, dec_degrees: 61.5 });
  });

  it('frames with the first rig that holds the project, moves the target by dragging the rectangle, and turns it by its handle', async () => {
    const { saves } = fixture(); mount(true, true, [rigB.rig.id, rigA.rig.id]);
    // rigB has no optics, so rigA frames by default; no click needed.
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61'));
    expect(screen.getByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35);
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    // Inside the rectangle (the mock puts its corners at ±2.69° × ±1.8° around the center): drag 100 px east.
    fireEvent.pointerDown(stage, pointer(512, 384));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 412, clientY: 384 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    const ra = Number((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value);
    expect(ra).toBeGreaterThan(seed.center.ra_degrees);
    expect(screen.getByTestId('framing-view-center')).toHaveTextContent('00h 42m 44.3s');
    // The handle sits past the top edge along the camera's up direction; dragging it due east of the center turns the camera to 90°.
    fireEvent.click(screen.getByRole('button', { name: 'Turn 90 degrees clockwise' }));
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(125);
    expect(screen.getByTestId('framing-rotate-handle')).toBeInTheDocument();
    const handle = document.querySelector('.framing-rotate circle')!;
    const hx = Number(handle.getAttribute('cx')); const hy = Number(handle.getAttribute('cy'));
    fireEvent.pointerDown(stage, pointer(hx, hy));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 300, clientY: 384 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    // The angle is read on the target's own plane. The stage projects about
    // the view center, which is still the seed, and from the moved target the
    // stage's horizontal is a shade off the target's east, so the reading is
    // what the model gives for that stage point: a little under 90°.
    const turned = () => Number((screen.getByLabelText('Position angle degrees') as HTMLInputElement).value);
    const center = { ra_degrees: Number((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value), dec_degrees: Number((screen.getByLabelText('Declination degrees') as HTMLInputElement).value) };
    const fov = Number((screen.getByLabelText('View width degrees') as HTMLInputElement).value);
    const expected = angleAt(center, skyAtStage(viewAt(seed.center, seed.center), fov, 300, 384));
    await waitFor(() => expect(turned()).toBeCloseTo(expected, 1));
    expect(turned()).toBeGreaterThan(88); expect(turned()).toBeLessThan(92);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].panel_rig_id).toBe(rigA.rig.id);
    expect(saves[0].position_angle_degrees).toBeCloseTo(turned(), 5);
  });

  it('frames a second rig on its own grid and angle over the same target, and saves it with the draft', async () => {
    const { saves } = fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    expect(screen.queryByTestId('framing-rig-panels')).not.toBeInTheDocument();
    // C925 has no optics, so its own framing starts from the shared size.
    const c925 = () => screen.getByRole('group', { name: 'C925 data' });
    fireEvent.click(within(c925()).getByRole('button', { name: 'Frame separately' }));
    // Its editor opens in its own card, and the panel tools work on its panels.
    expect(within(c925()).getByTestId('framing-own-rig')).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Mosaic panels · C925 data' })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('C925 data columns'), { target: { value: '3' } });
    fireEvent.click(screen.getByLabelText('C925 data follows the shared angle'));
    fireEvent.change(screen.getByLabelText('C925 data camera angle degrees'), { target: { value: '90' } });
    const own = () => screen.getByTestId('framing-panels');
    expect(own()).toHaveAttribute('data-framing', 'rig');
    expect(own().querySelectorAll('polygon')).toHaveLength(3);
    expect(own()).toHaveTextContent('r1c1');
    expect(screen.getByTestId('framing-own-extent')).toHaveTextContent('3 panels');
    // The shared framing is untouched and stays in sight: one panel for RedCat.
    expect(screen.getByTestId('framing-shared-panels').querySelectorAll('polygon')).toHaveLength(1);
    // Its center follows the shared one until it is given one of its own.
    expect(screen.getByLabelText('C925 data follows the shared center')).toBeChecked();
    expect(screen.getByLabelText('C925 data right ascension degrees')).toHaveValue(seed.center.ra_degrees);
    const before = own().querySelector('polygon')!.getAttribute('points');
    fireEvent.change(screen.getByLabelText('C925 data right ascension degrees'), { target: { value: '12' } });
    expect(screen.getByLabelText('C925 data follows the shared center')).not.toBeChecked();
    expect(own().querySelector('polygon')!.getAttribute('points')).not.toBe(before);
    // The shared target did not move with it.
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(seed.center.ra_degrees);
    fireEvent.click(within(c925()).getByRole('button', { name: 'Done' }));
    expect(screen.queryByTestId('framing-own-rig')).not.toBeInTheDocument();
    // Done, it is drawn in its own colour again, and the tools are the shared framing's.
    expect(screen.getByTestId('framing-rig-panels').querySelectorAll('polygon')).toHaveLength(3);
    expect(own()).toHaveAttribute('data-framing', 'shared');
    // Its card says what it shoots; the shared grid is still one panel.
    expect(c925()).toHaveTextContent('Separate framing · 1 × 3, 20% · 90° · moved center');
    expect(screen.getByLabelText('Mosaic columns')).toHaveValue(1);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].rig_framings).toEqual([{ rig_id: rigB.rig.id, center: { ra_degrees: 12, dec_degrees: seed.center.dec_degrees }, position_angle_degrees: 90, layout: gridLayout({ rows: 1, columns: 3, overlap_percent: 20 }), panel: { width_degrees: 5.38, height_degrees: 3.6 } }]);
    // One tick puts it back in step with the shared center.
    fireEvent.click(within(c925()).getByRole('button', { name: 'Edit' }));
    fireEvent.click(screen.getByLabelText('C925 data follows the shared center'));
    expect(screen.getByLabelText('C925 data right ascension degrees')).toHaveValue(seed.center.ra_degrees);
    // Back to the shared framing drops the rig's own grid.
    fireEvent.click(within(c925()).getByRole('button', { name: 'Use shared' }));
    expect(screen.queryByTestId('framing-rig-panels')).not.toBeInTheDocument();
    expect(within(c925()).getByRole('button', { name: 'Frame separately' })).toBeInTheDocument();
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
    // The search endpoint is asked for each spelling, but the online resolver only for the name as typed.
    fireEvent.change(screen.getByLabelText('Object name to resolve'), { target: { value: 'IC 1805' } });
    fireEvent.click(screen.getByRole('button', { name: 'Look up name' }));
    expect(await screen.findByLabelText('Target name')).toHaveValue('IC 1805');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2);
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61'));
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
    fixture();
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok(mosaic))));
    mount();
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
    // Another stack can be picked; a panel without it says so.
    const pick = screen.getByLabelText('Stack shown');
    expect([...pick.querySelectorAll('option')].map(option => option.textContent)).toEqual(['Best per panel', 'Ha', 'OIII']);
    fireEvent.change(pick, { target: { value: 'mono:OIII' } });
    expect(screen.getByText(/r1c1, RedCat 61: 36\/72 frames accepted; the OIII stack has no plate solve yet/)).toBeInTheDocument();
    expect(screen.queryByTestId('framing-stack')).not.toBeInTheDocument();
    expect(window.localStorage.getItem('psf-guard.framing.stack.project')).toBe('mono:OIII');
    fireEvent.change(pick, { target: { value: 'mono:Ha' } });
    expect(await screen.findByTestId('framing-stack')).toHaveAttribute('href', solvedStack.url);
    // The layer turns off from the sky's layer buttons, and stays off.
    fireEvent.click(screen.getByRole('button', { name: 'Finished stacks' }));
    expect(screen.queryByTestId('framing-stack')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Stack shown')).toBeDisabled();
    expect(window.localStorage.getItem('psf-guard.framing.stacks')).toBe('off');
    window.localStorage.removeItem('psf-guard.framing.stacks');
    window.localStorage.removeItem('psf-guard.framing.stack.project');
  });

  it('shows the stacks of a plan never activated, from its linked project', async () => {
    fixture();
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, activation_revision: null }))));
    mount();
    expect(await screen.findByTestId('framing-stack')).toHaveAttribute('href', solvedStack.url);
    expect(screen.getByRole('button', { name: 'Finished stacks' })).toHaveAttribute('aria-pressed', 'true');
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
    expect(screen.getByTestId('framing-view-center')).toHaveTextContent('20h 59m 00.0s');
    // Enter in the search box looked the name up; it did not save the draft.
    expect(saves).toHaveLength(0);
    // One click puts the previous target back, view included.
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(screen.getByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(seed.center.ra_degrees);
    expect(screen.getByTestId('framing-view-center')).toHaveTextContent('00h 42m 44.3s');
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
    // With nothing saved yet there is no saved framing to go back to.
    expect(screen.queryByRole('button', { name: 'Back to saved framing' })).not.toBeInTheDocument();
  });

  it('offers the local catalog names as they are typed and picks one without going online', async () => {
    const resolves: string[] = [];
    fixture();
    server.use(http.get('/api/director/v1/sky/resolve', ({ request }) => { resolves.push(new URL(request.url).searchParams.get('name') ?? ''); return HttpResponse.json({ success: false, data: null, error: 'offline' }, { status: 502 }); }));
    mount();
    await screen.findByLabelText('Target name');
    const box = screen.getByLabelText('Find a target');
    expect(box).toHaveAttribute('role', 'combobox');
    fireEvent.change(box, { target: { value: 'Nor' } });
    const list = await screen.findByRole('listbox', { name: 'Matching names' });
    await within(list).findByText('NGC 7000');
    const options = within(list).getAllByRole('option');
    expect(options[0]).toHaveTextContent('NGC 7000');
    expect(options[0]).toHaveTextContent('North America Nebula');
    expect(options[0]).toHaveTextContent('nebula');
    expect(options.at(-1)).toHaveTextContent('Look up “Nor” online');
    // Arrow down highlights the first name; Enter takes it.
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    expect(box).toHaveAttribute('aria-activedescendant', options[0].id);
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(await screen.findByText('Moved the target to NGC 7000.')).toBeInTheDocument();
    expect(screen.getByLabelText('Target name')).toHaveValue('NGC 7000');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(314.75);
    expect(screen.getByText(/NGC 7000 from Seiza object catalog/)).toBeInTheDocument();
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(resolves).toEqual([]);
    // A name typed in full is taken from the local list on Enter, with nothing highlighted.
    fireEvent.change(box, { target: { value: 'NGC 7023' } });
    await within(await screen.findByRole('listbox', { name: 'Matching names' })).findByText('Iris Nebula');
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(await screen.findByText('Moved the target to NGC 7023.')).toBeInTheDocument();
    expect(resolves).toEqual([]);
    // A name the local catalog does not know goes online.
    fireEvent.change(box, { target: { value: 'Zeta Nowhere' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    await waitFor(() => expect(resolves).toEqual(['Zeta Nowhere']));
  });

  it('goes back to the saved framing after a search, and only while something differs', async () => {
    const draft: DirectorFramingDraft = { project_id: 'project', revision: 3, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.45 }, position_angle_degrees: 15,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 8, updated_at_ms: 1 };
    fixture(draft); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('Heart');
    expect(screen.getByRole('button', { name: 'Back to saved framing' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Find a target'), { target: { value: 'NGC 7000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Go' }));
    expect(await screen.findByText('Moved the target to NGC 7000.')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Position angle degrees'), { target: { value: '90' } });
    const back = screen.getByRole('button', { name: 'Back to saved framing' });
    expect(back).toBeEnabled();
    fireEvent.click(back);
    expect(screen.getByText('Back to saved framing revision 3.')).toBeInTheDocument();
    expect(screen.getByLabelText('Target name')).toHaveValue('Heart');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(38.2);
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(15);
    expect(screen.getByRole('button', { name: 'Back to saved framing' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
  });

  const offlineSurveys = [
    { id: 'dss2_color', name: 'DSS2 color', hips: 'CDS/P/DSS2/color', kind: 'broadband', bandpass: 'Plates', attribution: 'DSS2 via CDS' },
    { id: 'nsns_ohs', name: 'NSNS SHO', hips: 'CDS/P/NSNS/OHS', kind: 'narrowband', bandpass: 'SHO', attribution: 'NSNS via CDS' },
    { id: 'nina:FramingAssistantCache', name: 'DSS (offline)', hips: '', kind: 'broadband', bandpass: 'Plates, offline', attribution: 'N.I.N.A. offline sky map', offline: true, stands_in_for: 'dss2_color' },
    { id: 'nina:FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars', name: 'NSNS SHO (offline)', hips: '', kind: 'narrowband', bandpass: 'SHO, offline', attribution: 'NSNS', offline: true, stands_in_for: 'nsns_ohs' },
  ];

  it('opens a draft saved on an online survey on the offline map that stands in for it', async () => {
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 2, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'nsns_ohs', view_fov_degrees: 6, updated_at_ms: 1 };
    const { cutouts } = fixture(stored);
    server.use(http.get('/api/director/v1/sky/surveys', () => HttpResponse.json(ok(offlineSurveys))));
    mount();
    await waitFor(() => expect(screen.getByLabelText('Survey')).toHaveValue('nina:FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars'));
    await waitFor(() => expect(cutouts.some(query => decodeURIComponent(query).includes('survey=nina:FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars&'))).toBe(true));
    expect(cutouts.some(query => query.includes('survey=nsns_ohs'))).toBe(false);
    // Once the view's own tile is up, wider views of the same place are fetched quietly, out to a hemisphere.
    await waitFor(() => expect(cutouts.some(query => query.includes('fov=180'))).toBe(true), { timeout: 8000 });
    // The settled 6° view's tile is 7.8° across; the prefetch steps up by four from there.
    expect(cutouts.some(query => query.includes('fov=31.2'))).toBe(true);
    // The online layer stays a click away.
    fireEvent.click(screen.getByRole('button', { name: 'SHO NSNS' }));
    expect(screen.getByLabelText('Survey')).toHaveValue('nsns_ohs');
  });

  it('starts a new framing on the offline DSS map when the server has one, and lists it first', async () => {
    const { cutouts } = fixture();
    server.use(http.get('/api/director/v1/sky/surveys', () => HttpResponse.json(ok(offlineSurveys.filter(s => s.id !== 'nsns_ohs')))));
    mount();
    await screen.findByLabelText('Target name');
    await waitFor(() => expect(screen.getByLabelText('Survey')).toHaveValue('nina:FramingAssistantCache'));
    const chips = within(screen.getByRole('group', { name: 'Survey layers' })).getAllByRole('button').map(button => button.textContent);
    expect(chips.slice(0, 2)).toEqual(['DSS (offline)', 'NSNS SHO (offline)']);
    await waitFor(() => expect(cutouts.some(query => decodeURIComponent(query).includes('survey=nina:FramingAssistantCache&'))).toBe(true));
    // A layer picked by hand stays put when the list refreshes.
    fireEvent.click(screen.getByRole('button', { name: 'DSS2' }));
    expect(screen.getByLabelText('Survey')).toHaveValue('dss2_color');
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

  it('moves the sky and the target together under a pinned rectangle, and Shift looks around', async () => {
    fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    fireEvent.click(screen.getByLabelText('Drag moves the sky'));
    expect(screen.getByLabelText('Drag moves the sky')).toHaveAttribute('aria-pressed', 'true');
    expect(window.localStorage.getItem('psf-guard.framing.dragMode')).toBe('sky');
    const ra = () => Number((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value);
    const viewLine = () => screen.getByTestId('framing-view-center').textContent!;
    const before = { ra: ra(), view: viewLine() };
    // Far from the rectangle: the sky moves, and the target with it, so the
    // rectangle stays put on the stage. Dragging the sky to the right brings
    // sky from the left, which is east, under the rectangle: RA grows.
    const polygonBefore = document.querySelector('.framing-panel polygon')!.getAttribute('points');
    fireEvent.pointerDown(stage, pointer(100, 700));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 200, clientY: 700 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(ra()).toBeGreaterThan(before.ra);
    expect(viewLine()).not.toBe(before.view);
    expect(document.querySelector('.framing-panel polygon')!.getAttribute('points')).toBe(polygonBefore);
    // Shift-drag looks around: the view moves and the target stays.
    const held = ra();
    fireEvent.pointerDown(stage, { ...pointer(100, 700), shiftKey: true });
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 200, clientY: 700 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(ra()).toBe(held);
    expect(document.querySelector('.framing-panel polygon')!.getAttribute('points')).not.toBe(polygonBefore);
  });

  it('turns the sky with the camera when asked, keeping the rectangle upright', async () => {
    fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(screen.getByTestId('framing-rotate-handle')).toBeInTheDocument());
    const compass = () => screen.getByTestId('framing-compass').getAttribute('data-rotation');
    expect(compass()).toBe('0.0');
    const handleBefore = document.querySelector('.framing-rotate circle')!;
    const cross = document.querySelector('.framing-target line')!;
    const cx = (Number(cross.getAttribute('x1')) + Number(cross.getAttribute('x2'))) / 2;
    // At 35° the handle leans east of straight up over a north-up sky.
    expect(Number(handleBefore.getAttribute('cx'))).toBeLessThan(cx - 10);
    fireEvent.click(screen.getByLabelText('Turn the sky with the camera'));
    expect(compass()).toBe('35.0');
    expect(window.localStorage.getItem('psf-guard.framing.rotateSky')).toBe('true');
    // Now the rectangle stands upright: the handle sits straight above the target.
    const handle = document.querySelector('.framing-rotate circle')!;
    expect(Number(handle.getAttribute('cx'))).toBeCloseTo(cx, 0);
    expect(screen.getByText(/sky turned 35.0°/)).toBeInTheDocument();
  });

  it('marks deep-sky objects, comets and planets from the catalogs, each layer with its own switch, and says what is missing', async () => {
    const asked: string[] = [];
    fixture();
    server.use(http.get('/api/director/v1/sky/objects', ({ request }) => {
      asked.push(new URL(request.url).search);
      return HttpResponse.json(ok({
        at_ms: 1_790_000_000_000, radius_degrees: 3,
        objects: { available: true, items: [
          { id: 'm31', name: 'M 31', common_name: 'Andromeda Galaxy', kind: 'galaxy', ra_degrees: seed.center.ra_degrees, dec_degrees: seed.center.dec_degrees, mag: 3.4, major_arcmin: 190, minor_arcmin: 60, position_angle_degrees: 35, prominence: 0.9 },
          { id: 'm32', name: 'M 32', common_name: '', kind: 'galaxy', ra_degrees: seed.center.ra_degrees + 0.2, dec_degrees: seed.center.dec_degrees - 0.4, mag: 8.1, major_arcmin: 8, minor_arcmin: 6, position_angle_degrees: null, prominence: 0.3 },
          { id: 'sh2-109', name: 'Sh2-109', common_name: '', kind: 'hii-region', ra_degrees: seed.center.ra_degrees - 0.3, dec_degrees: seed.center.dec_degrees + 0.2, mag: null, major_arcmin: 1080, minor_arcmin: null, position_angle_degrees: null, prominence: 0.29 },
        ] },
        minor_bodies: { available: false, note: 'minor-body catalog is not configured', items: [] },
        solar_system: [{ name: 'Jupiter', kind: 'planet', ra_degrees: seed.center.ra_degrees - 0.5, dec_degrees: seed.center.dec_degrees + 0.3, distance_au: 4.2, elongation_degrees: 120 }],
      }));
    }));
    mount();
    await waitFor(() => expect(screen.getByTestId('framing-marks')).toBeInTheDocument(), { timeout: 8000 });
    expect(screen.getAllByTestId('framing-mark-object')).toHaveLength(3);
    expect(screen.getByText('Andromeda Galaxy')).toBeInTheDocument();
    expect(screen.getByText('M 32')).toBeInTheDocument();
    // An object wider than the view is a dotted centre mark with its label beside it, not an arc across everything.
    const wider = screen.getAllByTestId('framing-mark-object')[2];
    expect(wider.getAttribute('class')).toContain('is-wider-than-view');
    expect(wider.querySelector('ellipse')!.getAttribute('rx')).toBe('10');
    const sh2 = screen.getByText('Sh2-109');
    expect(Number(sh2.getAttribute('x')) - Number(wider.querySelector('ellipse')!.getAttribute('cx'))).toBe(16);
    // Comets and the solar system wait for their switch.
    expect(screen.queryByTestId('framing-mark-solar')).not.toBeInTheDocument();
    expect(screen.queryByRole('note')).not.toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('Sun, Moon and planets'));
    fireEvent.click(screen.getByLabelText('Comets and asteroids'));
    expect(screen.getByTestId('framing-mark-solar')).toHaveTextContent('Jupiter');
    // The note on the sky is a few words; the reason is in its title.
    expect(screen.getByRole('note')).toHaveTextContent('No comet catalog');
    expect(screen.getByRole('note')).toHaveAttribute('title', 'Comets and asteroids need the Seiza minor-body catalog on this server (minor-body catalog is not configured).');
    expect(window.localStorage.getItem('psf-guard.framing.marks.bodies')).toBe('true');
    // The big galaxy is drawn at its catalog size and angle: 35° east of
    // north leans its major axis up and to the left on a north-up stage
    // (east is left), which in screen terms is 125° anticlockwise from x.
    const ellipse = screen.getAllByTestId('framing-mark-object')[0].querySelector('ellipse')!;
    expect(Number(ellipse.getAttribute('rx'))).toBeGreaterThan(Number(ellipse.getAttribute('ry')));
    const turned = Number(ellipse.getAttribute('transform')!.match(/rotate\((-?[\d.]+)\)/)![1]);
    expect(turned).toBeCloseTo(-125, 0);
    // Without a catalog angle the shape is a circle of the same area, not an ellipse along the screen.
    const unknown = screen.getAllByTestId('framing-mark-object')[1].querySelector('ellipse')!;
    expect(unknown.getAttribute('rx')).toBe(unknown.getAttribute('ry'));
    // The request names the settled view and a time rounded to ten minutes.
    expect(asked[0]).toMatch(/ra=10\.68&dec=41\.27&fov=/);
    expect(Number(asked[0].match(/at=(\d+)/)![1]) % 600_000).toBe(0);
    // Messier, NGC, IC, Sharpless, the Lynds catalogs and the supernova remnants start on; a chip adds a family and the choice is kept.
    expect(asked[0]).toContain('catalogs=M,NGC,IC,Sh,LDN,LBN,SNR');
    expect(asked[0]).not.toContain('hide=');
    expect(screen.getByRole('button', { name: 'PGC' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: 'NGC' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByRole('button', { name: 'PGC' }));
    await waitFor(() => expect(asked.at(-1)).toContain('catalogs=M,NGC,IC,Sh,LDN,LBN,SNR,PGC'), { timeout: 8000 });
    expect(window.localStorage.getItem('psf-guard.framing.marks.catalogs')).toBe('M,NGC,IC,Sh,LDN,LBN,SNR,PGC');
    fireEvent.click(screen.getByLabelText('Deep-sky marks'));
    expect(screen.queryByTestId('framing-mark-object')).not.toBeInTheDocument();
    expect(window.localStorage.getItem('psf-guard.framing.marks.objects')).toBe('false');
    fireEvent.click(screen.getByLabelText('Sun, Moon and planets'));
    expect(screen.queryByTestId('framing-mark-solar')).not.toBeInTheDocument();
  });

  it('keeps a picture behind the sky out to a hemisphere, with the grid and constellation names over it', async () => {
    const { cutouts } = fixture(); mount();
    // The first tile answers 202 once and then arrives; under a loaded suite that takes a while.
    await waitFor(() => expect(screen.getByTestId('framing-sky')).toBeInTheDocument(), { timeout: 10_000 });
    expect(screen.getByTestId('framing-graticule').querySelectorAll('path').length).toBeGreaterThan(3);
    expect(screen.queryByTestId('framing-names')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('View width degrees'), { target: { value: '120' } });
    await waitFor(() => expect(screen.getByTestId('framing-names')).toBeInTheDocument(), { timeout: 10_000 });
    // A wide view still asks for a picture: the widest the server renders.
    await waitFor(() => expect(cutouts.some(query => query.includes('fov=180'))).toBe(true), { timeout: 10_000 });
    // The survey shows the real stars, so the drawn ones stay off while a picture is up.
    await waitFor(() => expect(screen.getByTestId('framing-sky')).toBeInTheDocument(), { timeout: 10_000 });
    expect(screen.queryByTestId('framing-stars')).not.toBeInTheDocument();
    // The rectangle is still drawn, tiny, and the readout gives the sky width the stage really spans.
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    expect(screen.getByText(/across · N up, E left/).textContent).toMatch(/^1[0-3]\d\.\d+° across/);
    fireEvent.click(screen.getByLabelText('Equatorial grid'));
    expect(screen.queryByTestId('framing-graticule')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Zoom in' }));
    expect(screen.getByLabelText('View width degrees')).toHaveValue(80);
  });

  it('does not call a framing edited when only the view stood in a survey for the saved one', async () => {
    // A draft saved on an online survey opens on the offline map that
    // stands in for it: that is the view's choice, not an unsaved edit.
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 2, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'nsns_ohs', view_fov_degrees: 6, updated_at_ms: 1 };
    fixture(stored);
    server.use(http.get('/api/director/v1/sky/surveys', () => HttpResponse.json(ok(offlineSurveys))));
    const drafts = mountOnPage();
    await waitFor(() => expect(screen.getByLabelText('Survey')).toHaveValue('nina:FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars'));
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    expect(drafts.current!.unsaved).toHaveLength(0);

    // A real edit is unsaved, and says what it is.
    fireEvent.change(screen.getByLabelText('Position angle degrees'), { target: { value: '15' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(drafts.current!.unsaved[0].changes).toEqual(['camera angle 0° → 15°']);
  });

  it('keeps a survey picked by hand in this browser and never calls it an edit', async () => {
    // The layer is how the sky is shown, not part of what is planned.
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 2, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 6, updated_at_ms: 1 };
    fixture(stored);
    const drafts = mountOnPage();
    await waitFor(() => expect(screen.getByLabelText('Survey')).toHaveValue('dss2_color'));
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    fireEvent.change(screen.getByLabelText('Survey'), { target: { value: 'finkbeiner_halpha' } });
    expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha');
    expect(drafts.current!.unsaved).toHaveLength(0);
    expect(screen.getByRole('button', { name: 'Back to saved framing' })).toBeDisabled();
    expect(window.localStorage.getItem('psf-guard.framing.survey')).toBe('finkbeiner_halpha');

    // Opened again, this plan or another shows the layer this browser picked.
    cleanup();
    fixture(stored);
    const again = mountOnPage();
    await waitFor(() => expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha'));
    await waitFor(() => expect(again.current?.sections.map(section => section.id)).toEqual(['framing']));
    expect(again.current!.unsaved).toHaveLength(0);
  });

  it('shows coordinates and angles to the places that matter, and saves them whole', async () => {
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 2, target_name: 'Medusa Nebula', center: { ra_degrees: 112.26128933333334, dec_degrees: 13.246830369444444 }, position_angle_degrees: 218.8412345,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: null, panel: { width_degrees: 0.5727744486363814, height_degrees: 0.3828274803946109 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 1.6164391178182103, updated_at_ms: 1 };
    const { saves } = fixture(stored);
    mount();
    expect(await screen.findByLabelText('Right ascension degrees')).toHaveValue(112.26129);
    expect(screen.getByLabelText('Declination degrees')).toHaveValue(13.24683);
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(218.84);
    expect(screen.getByLabelText('Panel width degrees')).toHaveValue(0.573);
    expect(screen.getByLabelText('View width degrees')).toHaveValue(1.62);
    fireEvent.change(screen.getByLabelText('Target name'), { target: { value: 'Medusa' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].center).toEqual(stored.center);
    expect(saves[0].position_angle_degrees).toBe(218.8412345);
  });

  it('picks the panel rig from the swatch beside its name, and shows which one is active', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107',
      field_of_view: { width_degrees: 1.2, height_degrees: 0.8, pixel_scale_arcsec: 1.9, focal_ratio: 7 } };
    fixture();
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    // Askar 107 is not in this plan: it still shows, open, and can be picked.
    mount(true, true, [rigA.rig.id]);
    const active = await screen.findByRole('button', { name: 'RedCat 61 frames the plan' });
    expect(screen.getByText(/^Other rigs/).closest('details')).toHaveAttribute('open');

    expect(active).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByRole('button', { name: 'Use for panel size' })).not.toBeInTheDocument();
    // A rig without optics has nothing to pick.
    expect(screen.queryByRole('button', { name: /C925 data/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Frame with Askar 107' }));
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from Askar 107');
    expect(screen.queryByLabelText('Panel rig')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Askar 107 frames the plan' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Frame with RedCat 61' })).toHaveAttribute('aria-pressed', 'false');

    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 1.20° × 48.0′');
    // Typing a size keeps the rig's numbers to start from, and drops the pick.
    expect(screen.queryByLabelText('Panel width degrees')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Type a size' }));
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Typed size · or pick a rig by its swatch');
    expect(screen.getByLabelText('Panel width degrees')).toHaveValue(1.2);
    expect(screen.getByRole('button', { name: 'Frame with Askar 107' })).toHaveAttribute('aria-pressed', 'false');
    fireEvent.change(screen.getByLabelText('Panel width degrees'), { target: { value: '2' } });
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 2.00° × 48.0′');
  });

  it('turns several rigs on and off with their switches, and frames with one that is on', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107' };
    fixture();
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    const toggled: Array<[string, boolean]> = [];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const view = (shooting: string[]) => <QueryClientProvider client={client}><FramingView projectId="project" seed={seed} preferredRigIds={[rigA.rig.id]}
      shootingRigIds={shooting} onToggleRig={(id, on) => { toggled.push([id, on]); }} /></QueryClientProvider>;
    const { rerender } = render(view([rigA.rig.id]));
    const redcat = await screen.findByRole('switch', { name: 'RedCat 61 on' });
    expect(redcat).toHaveAttribute('aria-checked', 'true');
    // Rigs are grouped On and Off.
    expect(screen.getByRole('heading', { name: 'On' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Off' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Askar 107' })).toHaveTextContent('Not in this plan');
    // A rig that is off cannot frame the plan.
    expect(screen.queryByRole('button', { name: 'Frame with Askar 107' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch', { name: 'Askar 107 on' }));
    // Turning a rig on frames the plan with it at once.
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from Askar 107');
    fireEvent.click(screen.getByRole('switch', { name: 'C925 data on' }));
    // C925 has no optics, so it joins without taking the framing over.
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from Askar 107');
    expect(toggled).toEqual([[rigC.rig.id, true], [rigB.rig.id, true]]);
    rerender(view([rigA.rig.id, rigC.rig.id, rigB.rig.id]));
    expect(screen.getByRole('switch', { name: 'Askar 107 on' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByRole('heading', { name: 'Off' })).not.toBeInTheDocument();
    // One rig frames the plan; any other that is on can take over by its heading.
    expect(screen.getByRole('button', { name: 'Askar 107 frames the plan' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(screen.getByRole('button', { name: 'Frame with RedCat 61' })).getByText('RedCat 61')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Frame with RedCat 61' }));
    expect(screen.getByRole('button', { name: 'RedCat 61 frames the plan' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByRole('button', { name: 'Frame with Askar 107' }));
    expect(screen.getByRole('group', { name: 'Askar 107' })).toHaveClass('is-framing');
    fireEvent.click(screen.getByRole('switch', { name: 'Askar 107 on' }));
    expect(toggled.at(-1)).toEqual([rigC.rig.id, false]);
  });

  it('frames with a rig turned on only when it joins the plan', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107' };
    fixture();
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    const toggled: Array<[string, boolean]> = [];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    // No exposure template matches Askar, so the page says it did not join.
    render(<QueryClientProvider client={client}><FramingView projectId="project" seed={seed} preferredRigIds={[rigA.rig.id]}
      shootingRigIds={[rigA.rig.id]} onToggleRig={(id, on) => { toggled.push([id, on]); return false; }} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61'));
    fireEvent.click(screen.getByRole('switch', { name: 'Askar 107 on' }));
    expect(toggled).toEqual([[rigC.rig.id, true]]);
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61');
    expect(screen.getByRole('button', { name: 'RedCat 61 frames the plan' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('hides the shared framing when every rig that is on frames separately', async () => {
    fixture();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}><FramingView projectId="project" seed={seed} preferredRigIds={[rigB.rig.id]}
      shootingRigIds={[rigB.rig.id]} onToggleRig={() => {}} /></QueryClientProvider>);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    // Only C925's own panels are left, being edited: no rig shoots the shared rectangle.
    expect(screen.getByTestId('framing-panels')).toHaveAttribute('data-framing', 'rig');
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    expect(screen.queryByTestId('framing-shared-panels')).not.toBeInTheDocument();
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Done' }));
    expect(screen.getByTestId('framing-rig-panels').querySelectorAll('polygon')).toHaveLength(1);
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(0);
  });

  it('does not call the same rigs in another order an edit', async () => {
    // Unticking a rig and ticking it again puts it last. The bar would have
    // nothing to list, so it must not say the framing is unsaved.
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107' };
    const stored: DirectorFramingDraft = { project_id: 'project', revision: 2, target_name: 'M33', center: { ra_degrees: 23.46, dec_degrees: 30.66 }, position_angle_degrees: 0,
      layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 }, shown_rig_ids: [rigA.rig.id, rigC.rig.id], survey_id: 'dss2_color', view_fov_degrees: 6, updated_at_ms: 1 };
    fixture(stored);
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigC]))));
    const drafts = mountOnPage();
    const redcat = await screen.findByRole('checkbox', { name: /RedCat 61/ });
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    fireEvent.click(redcat);
    await waitFor(() => expect(drafts.current!.unsaved[0]?.changes).toEqual(['rigs compared']));
    fireEvent.click(redcat);
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(0));
  });

  it('saves a framing taken from the catalog quietly, never as an unsaved edit', async () => {
    // No saved draft: the view starts from the catalog target and picks a
    // panel rig. Nothing to warn about, but activation needs it stored, so
    // the next save stores it.
    const { saves } = fixture(null);
    const drafts = mountOnPage();
    expect(await screen.findByLabelText('Target name')).toHaveValue('M31');
    await waitFor(() => expect(drafts.current?.sections[0]?.pending).toBe(true));
    expect(drafts.current!.unsaved).toHaveLength(0);
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves).toHaveLength(1);
    expect(saves[0].panel_rig_id).toBe(rigA.rig.id);
    await waitFor(() => expect(drafts.current!.sections[0].pending).toBe(false));
  });

  /** The stage as the drag tests lay it out: 1024 × 768 at the window's corner. */
  const laidOutStage = () => {
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    return stage;
  };
  const valueOf = (label: string) => Number((screen.getByLabelText(label) as HTMLInputElement).value);
  const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107',
    field_of_view: { width_degrees: 1.2, height_degrees: 0.8, pixel_scale_arcsec: 1.9, focal_ratio: 7 } };
  const heart: DirectorFramingDraft = { project_id: 'project', revision: 3, target_name: 'Heart', center: { ra_degrees: 38.2, dec_degrees: 61.5 }, position_angle_degrees: 0,
    layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 20 }), panel_rig_id: null, panel: { width_degrees: 2, height_degrees: 1.5 }, shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 6, updated_at_ms: 1 };
  /** The view on the planning page, with the rigs that shoot the plan. */
  function mountWithRigs(shooting: string[]) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const view = (on: string[]) => <QueryClientProvider client={client}><FramingView projectId="project" seed={seed} shootingRigIds={on} onToggleRig={() => {}} /></QueryClientProvider>;
    const { rerender, unmount } = render(view(shooting));
    return { rerender: (on: string[]) => rerender(view(on)), unmount };
  }

  it('turns the camera by its handle over a turned sky without spinning it', async () => {
    fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(screen.getByTestId('framing-rotate-handle')).toBeInTheDocument());
    const stage = laidOutStage();
    fireEvent.click(screen.getByLabelText('Turn the sky with the camera'));
    const circle = document.querySelector('.framing-rotate circle')!;
    const [hx, hy] = [Number(circle.getAttribute('cx')), Number(circle.getAttribute('cy'))];
    // Ten small moves. Each is read against the view the drag began in; read
    // through a view the sky turns by the angle being changed, they spun the
    // camera from 35° to 331.8°.
    fireEvent.pointerDown(stage, pointer(hx, hy));
    for (let step = 1; step <= 10; step += 1) fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: hx + 3 * step, clientY: hy });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    const expected = angleAt(seed.center, skyAtStage(viewAt(seed.center, seed.center, 35), valueOf('View width degrees'), hx + 30, hy));
    expect(valueOf('Position angle degrees')).toBeCloseTo(expected, 0);
    expect(Math.abs(valueOf('Position angle degrees') - 35)).toBeLessThan(20);
    // Fit takes the turned rectangle and its handle: the handle stays on the stage.
    fireEvent.click(screen.getByRole('button', { name: 'Fit the footprint' }));
    const fitted = document.querySelector('.framing-rotate circle')!;
    expect(Number(fitted.getAttribute('cx'))).toBeGreaterThan(0); expect(Number(fitted.getAttribute('cx'))).toBeLessThan(1024);
    expect(Number(fitted.getAttribute('cy'))).toBeGreaterThan(0); expect(Number(fitted.getAttribute('cy'))).toBeLessThan(768);
  });

  it('keeps a panned view and an edit typed while a save is out', async () => {
    const { saves } = fixture(heart);
    let answer!: () => void;
    const answered = new Promise<void>(resolve => { answer = resolve; });
    server.use(http.put('/api/director/v1/projects/project/framing', async ({ request }) => {
      const body = await request.json() as DirectorFramingDraft;
      saves.push(body);
      await answered;
      return HttpResponse.json(ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft: { ...body, revision: body.revision + 1, updated_at_ms: 5 } }));
    }));
    const drafts = mountOnPage();
    expect(await screen.findByLabelText('Target name')).toHaveValue('Heart');
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    // Look away from the target, and turn the camera: an edit to save.
    const stage = laidOutStage();
    fireEvent.pointerDown(stage, pointer(100, 700));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 300, clientY: 600 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    const panned = screen.getByTestId('framing-view-center').textContent!;
    expect(panned).not.toContain('02h 32m 48.0s');
    fireEvent.change(screen.getByLabelText('Position angle degrees'), { target: { value: '10' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    const saving = drafts.current!.saveAll();
    await waitFor(() => expect(saves).toHaveLength(1));
    fireEvent.change(screen.getByLabelText('Target name'), { target: { value: 'Heart and Soul' } });
    answer();
    expect(await saving).toBeNull();
    expect(saves[0].position_angle_degrees).toBe(10);
    // The view stays where it was looked at, and the name typed meanwhile is still an edit.
    await waitFor(() => expect(drafts.current!.unsaved[0]?.changes).toEqual(['target name “Heart” → “Heart and Soul”']));
    expect(screen.getByTestId('framing-view-center')).toHaveTextContent(panned);
    expect(screen.getByLabelText('Target name')).toHaveValue('Heart and Soul');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(10);
  });

  it('hands the panel size to a rig that is on once its rig goes off, and says when none can take it', async () => {
    fixture({ ...heart, panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 } });
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    const source = () => screen.getByTestId('framing-panel-source');
    const view = mountWithRigs([rigA.rig.id, rigC.rig.id]);
    await waitFor(() => expect(source()).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°'));
    // Turned off, here or on another tab: the rig that is on takes over.
    view.rerender([rigC.rig.id]);
    await waitFor(() => expect(source()).toHaveTextContent('Size from Askar 107 · 1.20° × 48.0′'));
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 1.20° × 48.0′');
    // C925 knows no optics, so with only it on nothing can take over.
    view.rerender([rigB.rig.id]);
    expect(source()).toHaveTextContent('Size from Askar 107 (off) · 1.20° × 48.0′');
    view.unmount();
    // A saved panel rig that is off when the view opens gives way at once.
    mountWithRigs([rigC.rig.id]);
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from Askar 107 · 1.20° × 48.0′'));
  });

  it('says at the field what the server would refuse, and holds the save until it is fixed', async () => {
    const { saves } = fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61'));
    // Eight RedCat columns at 20% overlap span 35.5°; the core takes 30°.
    fireEvent.change(screen.getByLabelText('Mosaic columns'), { target: { value: '8' } });
    expect(screen.getByTestId('framing-problem')).toHaveTextContent('Mosaic 35.51° × 3.60°, over 30°');
    expect(screen.getByLabelText('Mosaic columns')).toHaveAttribute('aria-invalid', 'true');
    const button = () => screen.getByRole('button', { name: 'Save framing' });
    expect(button()).toBeDisabled();
    expect(button()).toHaveAttribute('title', 'Mosaic 35.51° × 3.60°, over 30°');
    fireEvent.change(screen.getByLabelText('Mosaic columns'), { target: { value: '2' } });
    fireEvent.click(screen.getByRole('button', { name: 'Type a size' }));
    fireEvent.change(screen.getByLabelText('Panel width degrees'), { target: { value: '40' } });
    expect(screen.getByTestId('framing-problem')).toHaveTextContent('Panel sides 0.01° to 30°');
    expect(screen.getByLabelText('Panel width degrees')).toHaveAttribute('aria-invalid', 'true');
    expect(button()).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Panel width degrees'), { target: { value: '5' } });
    // A right ascension of 360 is 0, and a declination past the pole is the pole.
    fireEvent.change(screen.getByLabelText('Right ascension degrees'), { target: { value: '360' } });
    fireEvent.change(screen.getByLabelText('Declination degrees'), { target: { value: '95' } });
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(0);
    expect(screen.getByLabelText('Declination degrees')).toHaveValue(90);
    expect(screen.queryByTestId('framing-problem')).not.toBeInTheDocument();
    fireEvent.click(button());
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ center: { ra_degrees: 0, dec_degrees: 90 }, layout: { panels: [{ id: 'r1c1' }, { id: 'r1c2' }] }, panel: { width_degrees: 5 } });
  });

  it('gives the page\'s save bar the reason a framing cannot be saved', async () => {
    const { saves } = fixture(heart);
    const drafts = mountOnPage();
    await screen.findByLabelText('Target name');
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    fireEvent.change(screen.getByLabelText('Panel width degrees'), { target: { value: '0' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(await drafts.current!.saveAll()).toEqual({ label: 'Framing', reason: 'Panel sides 0.01° to 30°' });
    expect(saves).toHaveLength(0);
  });

  it('calls a quarter turn and back no change', async () => {
    fixture({ ...heart, position_angle_degrees: 35.1 });
    const drafts = mountOnPage();
    await screen.findByLabelText('Target name');
    await waitFor(() => expect(drafts.current?.sections.map(section => section.id)).toEqual(['framing']));
    fireEvent.click(screen.getByRole('button', { name: 'Turn 90 degrees clockwise' }));
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(125.1);
    await waitFor(() => expect(drafts.current!.unsaved[0]?.changes).toEqual(['camera angle 35.1° → 125.1°']));
    fireEvent.click(screen.getByRole('button', { name: 'Turn 90 degrees counter-clockwise' }));
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(0));
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35.1);
  });

  it('keeps the framing on screen when a reload fails, and offers to try again', async () => {
    fixture(heart); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('Heart');
    fireEvent.change(screen.getByLabelText('Target name'), { target: { value: 'Heart edited' } });
    server.use(http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json({ success: false, data: null, error: 'The store is down' }, { status: 500 })));
    fireEvent.click(screen.getByRole('button', { name: 'Reload framing' }));
    const banner = await screen.findByText(/The framing could not be reloaded/);
    expect(screen.getByLabelText('Target name')).toHaveValue('Heart edited');
    expect(screen.getByTestId('framing-stage')).toBeInTheDocument();
    server.use(http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft: heart }))));
    fireEvent.click(within(banner).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.queryByText(/The framing could not be reloaded/)).not.toBeInTheDocument());
    // Nothing was saved elsewhere, so the edit stays.
    expect(screen.getByLabelText('Target name')).toHaveValue('Heart edited');
  });

  it('gives a hidden shared framing no handle to turn', async () => {
    fixture();
    mountWithRigs([rigB.rig.id]);
    await waitFor(() => expect(screen.getByTestId('framing-rotate-handle')).toBeInTheDocument());
    const circle = document.querySelector('.framing-rotate circle')!;
    const [hx, hy] = [Number(circle.getAttribute('cx')), Number(circle.getAttribute('cy'))];
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    expect(screen.queryByTestId('framing-rotate-handle')).not.toBeInTheDocument();
    const stage = laidOutStage();
    fireEvent.pointerDown(stage, pointer(hx, hy));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: hx + 200, clientY: hy + 100 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35);
  });

  it('zooms by how far the wheel turned, so a trackpad swipe is no leap', async () => {
    fixture(heart); mount();
    await screen.findByLabelText('Target name');
    const stage = screen.getByTestId('framing-stage');
    expect(valueOf('View width degrees')).toBe(6);
    // Fifty small trackpad steps of 4 pixels are two mouse notches: 1.2 × 1.2.
    for (let step = 0; step < 50; step += 1) fireEvent.wheel(stage, { deltaY: 4, deltaMode: 0 });
    expect(valueOf('View width degrees')).toBeCloseTo(6 * 1.44, 1);
    // Lines count 16 pixels; a page-long spin zooms at most 2 times per event.
    fireEvent.wheel(stage, { deltaY: -3, deltaMode: 1 });
    expect(valueOf('View width degrees')).toBeCloseTo(6 * 1.44 / Math.pow(1.2, 0.48), 1);
    const before = valueOf('View width degrees');
    fireEvent.wheel(stage, { deltaY: 10_000, deltaMode: 0 });
    expect(valueOf('View width degrees')).toBeCloseTo(before * 2, 1);
  });

  it('undoes a found target and nothing after it, and carries a moved separate framing along', async () => {
    fixture(); mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    fireEvent.change(screen.getByLabelText('C925 data right ascension degrees'), { target: { value: '11.6847' } });
    fireEvent.change(screen.getByLabelText('Find a target'), { target: { value: 'NGC 7000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Go' }));
    expect(await screen.findByText('Moved the target to NGC 7000.')).toBeInTheDocument();
    // The separate framing keeps its place beside the target, not at M31.
    expect(valueOf('C925 data right ascension degrees')).toBeGreaterThan(314.75);
    expect(valueOf('C925 data right ascension degrees')).toBeLessThan(317);
    // An edit after the pick survives its Undo.
    fireEvent.change(screen.getByLabelText('Position angle degrees'), { target: { value: '80' } });
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(screen.getByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Right ascension degrees')).toHaveValue(seed.center.ra_degrees);
    expect(screen.getByTestId('framing-view-center')).toHaveTextContent('00h 42m 44.3s');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(80);
    expect(valueOf('C925 data right ascension degrees')).toBeCloseTo(11.6847, 4);
  });

  it('shows the size the rectangle uses, and offers the rig\'s field once it has moved', async () => {
    fixture({ ...heart, panel_rig_id: rigA.rig.id, panel: { width_degrees: 5, height_degrees: 3.5 } }); mount();
    const source = () => screen.getByTestId('framing-panel-source');
    await waitFor(() => expect(source()).toHaveTextContent('Size from RedCat 61 · 5.00° × 3.50°'));
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 5.00° × 3.50°');
    fireEvent.click(screen.getByRole('button', { name: 'Use current field' }));
    expect(source()).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°');
    expect(screen.queryByRole('button', { name: 'Use current field' })).not.toBeInTheDocument();
  });

  it('keeps the size a rig gave after it lost its optics', async () => {
    fixture({ ...heart, panel_rig_id: rigA.rig.id, panel: { width_degrees: 5, height_degrees: 3.5 } });
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ ...rigA, field_of_view: null }, rigB]))));
    mount();
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 · 5.00° × 3.50° · no optics now'));
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel · 5.00° × 3.50°');
  });

  it('dims the separate framing of a rig that is off and leaves it in place, and lists one whose rig is gone', async () => {
    const gone = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd';
    fixture({ ...heart, center: seed.center, view_fov_degrees: 20, panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 }, rig_framings: [
      { rig_id: rigB.rig.id, center: { ra_degrees: seed.center.ra_degrees, dec_degrees: seed.center.dec_degrees + 4 }, position_angle_degrees: 0, layout: gridLayout({ rows: 1, columns: 1, overlap_percent: 0 }), panel: { width_degrees: 1, height_degrees: 1 } },
      { rig_id: gone, center: null, position_angle_degrees: null, layout: gridLayout({ rows: 1, columns: 2, overlap_percent: 0 }), panel: { width_degrees: 1, height_degrees: 1 } },
    ] });
    mountWithRigs([rigA.rig.id]);
    await waitFor(() => expect(screen.getAllByTestId('framing-rig-panels')).toHaveLength(2));
    for (const group of screen.getAllByTestId('framing-rig-panels')) expect(group).toHaveClass('is-off');
    // Off, it stays where it is: a drag on it looks around instead.
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Edit' }));
    const at = valueOf('C925 data declination degrees');
    const stage = laidOutStage();
    const polygon = document.querySelector(`[data-rig="${rigB.rig.id}"] polygon`)!;
    const points = polygon.getAttribute('points')!.split(' ').map(pair => pair.split(',').map(Number));
    const [x, y] = [points.reduce((sum, [px]) => sum + px, 0) / 4, points.reduce((sum, [, py]) => sum + py, 0) / 4];
    fireEvent.pointerDown(stage, pointer(x, y));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: x + 100, clientY: y + 50 });
    fireEvent.pointerUp(stage, { pointerId: 1 });
    expect(valueOf('C925 data declination degrees')).toBe(at);
    expect(screen.getByTestId('framing-view-center')).not.toHaveTextContent('00h 42m 44.3s');
    // A rig gone from the list has no card; its framing is listed so it can go.
    const orphans = screen.getByRole('list', { name: 'Framings of rigs not listed' });
    expect(orphans).toHaveTextContent('Rig not listed · separate framing, 2 panels');
    fireEvent.click(within(orphans).getByRole('button', { name: 'Remove' }));
    expect(screen.queryByRole('list', { name: 'Framings of rigs not listed' })).not.toBeInTheDocument();
    expect(screen.getAllByTestId('framing-rig-panels')).toHaveLength(1);
  });

  it('folds the survey chips behind one on a narrow stage', async () => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    const measured = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const [width, height] = this.dataset.testid === 'framing-stage' ? [358, 300] : [0, 0];
      return { left: 0, top: 0, width, height, right: width, bottom: height, x: 0, y: 0, toJSON: () => ({}) };
    });
    try {
      fixture(heart); mount();
      const layers = await screen.findByRole('button', { name: 'Survey layers' });
      expect(layers).toHaveAttribute('aria-expanded', 'false');
      expect(layers).toHaveTextContent('DSS2');
      expect(screen.queryByRole('group', { name: 'Survey layers' })).not.toBeInTheDocument();
      fireEvent.click(layers);
      const chips = screen.getByRole('group', { name: 'Survey layers' });
      fireEvent.click(within(chips).getByRole('button', { name: 'Hα Finkbeiner' }));
      expect(screen.queryByRole('group', { name: 'Survey layers' })).not.toBeInTheDocument();
      expect(screen.getByLabelText('Survey')).toHaveValue('finkbeiner_halpha');
    } finally { measured.mockRestore(); }
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
    expect(stackMatrix({ ...solvedStack, wcs: null }, viewAt(view, view), 2)).toBeNull();
  });

  it('maps offsets to the stage with north up and east left', () => {
    expect(toStage([0, 0], 4)).toEqual([512, 384]);
    const [x, y] = toStage([1, 0.5], 4);
    expect(x).toBeLessThan(512);
    expect(y).toBeLessThan(384);
    // The stage is stereographic: a degree on the plane is a hair under a degree of sky.
    const east = skyAtStage(viewAt({ ra_degrees: 10, dec_degrees: 0 }, { ra_degrees: 10, dec_degrees: 0 }), 4, 256, 384);
    expect(east.ra_degrees).toBeCloseTo(11, 3);
    expect(moveBy({ ra_degrees: 359.5, dec_degrees: 89.9 }, 1, 1).dec_degrees).toBe(90);
  });
});

describe('Mosaic panels', () => {
  beforeEach(() => {
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:stage'), revokeObjectURL: vi.fn() }));
    window.localStorage.clear();
  });
  afterEach(() => vi.unstubAllGlobals());

  /** M31 framed with RedCat at 35°, its panels as given. */
  const placed = (panels: Array<{ id: string; x: number; y: number; locked: boolean; unlocked_by_hand?: boolean }>): DirectorFramingDraft => ({ project_id: 'project', revision: 3, target_name: 'M31', center: seed.center,
    position_angle_degrees: 35, layout: { min_overlap_percent: 20, panels }, panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 },
    shown_rig_ids: [], survey_id: 'dss2_color', view_fov_degrees: 20, updated_at_ms: 1 });
  const stageOf = () => {
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 1024, configurable: true });
    return stage;
  };
  /** A panel's middle on the stage, from its drawn corners. */
  const middle = (id: string): [number, number] => {
    const points = document.querySelector(`[data-panel-id='${id}'] polygon`)!.getAttribute('points')!.split(' ').map(pair => pair.split(',').map(Number));
    return [points.reduce((sum, [x]) => sum + x, 0) / 4, points.reduce((sum, [, y]) => sum + y, 0) / 4];
  };

  it('holds a panel with frames where they were shot and adds panels round it', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    // The framed target has 40 frames: it is locked, and so are the center and angle.
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, panels: [mosaic.panels[0]] }))));
    mount();
    await waitFor(() => expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-panel-state', 'locked'));
    expect(screen.getByLabelText('Right ascension degrees')).toBeDisabled();
    expect(screen.getByLabelText('Position angle degrees')).toBeDisabled();
    expect(screen.getByTestId('framing-held')).toHaveTextContent('Locked panels hold the center and camera angle');
    expect(screen.queryByLabelText('Mosaic rows')).not.toBeInTheDocument();
    expect(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ })).toHaveTextContent('40 frames');
    // A column east, then a row north across both.
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    expect(await screen.findByText(/Added p2 on the E side/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: "Add a row on the mosaic's north side" }));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(4);
    expect(screen.queryByTestId('framing-issues')).not.toBeInTheDocument();
    // Undo takes the row back, redo puts it again.
    fireEvent.click(screen.getByRole('button', { name: 'Undo panel change' }));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2);
    fireEvent.click(screen.getByRole('button', { name: 'Redo panel change' }));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(4);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    const [first, east, ...north] = saves[0].layout.panels;
    expect(first).toEqual({ id: 'r1c1', x: 0, y: 0, locked: true });
    // At 35° east is the camera's left: 80% of a panel that way.
    expect(east.id).toBe('p2'); expect(east.x).toBeCloseTo(-0.8, 9); expect(east.y).toBe(0);
    expect(north.map(panel => panel.y.toFixed(3))).toEqual(['0.800', '0.800']);
    expect(saves[0].center).toEqual(seed.center);
  });

  it('drags one panel, settles it at the minimum, refuses a thin overlap and places it with Alt', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    fireEvent.click(screen.getByRole('button', { name: 'Drag moves one panel' }));
    const stage = stageOf();
    // The locked panel stays put however it is dragged: the drag looks
    // around instead, so both panels slide together.
    const [lx, ly] = middle('r1c1');
    const apart = middle('p2')[0] - lx;
    fireEvent.pointerDown(stage, pointer(lx, ly));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: lx + 60, clientY: ly });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: lx + 60, clientY: ly });
    expect(Math.abs(middle('p2')[0] - middle('r1c1')[0] - apart)).toBeLessThan(5);
    expect(screen.getByText(/r1c1 is locked/)).toBeInTheDocument();
    // From the locked panel's middle to p2's is 80% of a panel width on the stage.
    const [ax, ay] = middle('r1c1');
    const [bx, by] = middle('p2');
    const along = [(bx - ax) / 0.8, (by - ay) / 0.8];
    // A little further out is pulled back to the minimum overlap.
    const nudge = (share: number) => [bx + along[0] * share, by + along[1] * share];
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: nudge(0.02)[0], clientY: nudge(0.02)[1] });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: nudge(0.02)[0], clientY: nudge(0.02)[1] });
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    // Well past it leaves 8% overlap: refused, and the panel goes back.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1] });
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'invalid');
    expect(screen.getByTestId('framing-overlap')).toHaveAttribute('data-short', 'true');
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1] });
    expect(screen.getByText(/Not placed: r1c1 and p2 overlap by only 8%/)).toBeInTheDocument();
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    // Alt places it anyway, flagged.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1], altKey: true });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1], altKey: true });
    expect(screen.getByText(/Placed p2 off the rule/)).toBeInTheDocument();
    expect(screen.getByTestId('framing-issues')).toHaveTextContent('r1c1 and p2 overlap by only 8%, under the 20% minimum');
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].layout.panels[1].x).toBeCloseTo(-0.92, 2);
  });

  it('with Snap off, leaves a dragged panel where it is dropped and flags the mosaic', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    fireEvent.click(screen.getByRole('button', { name: 'Drag moves one panel' }));
    fireEvent.click(screen.getByRole('button', { name: 'Snap panels' }));
    expect(screen.getByRole('button', { name: 'Snap panels' })).toHaveAttribute('aria-pressed', 'false');
    const stage = stageOf();
    const [ax, ay] = middle('r1c1');
    const [bx, by] = middle('p2');
    const along = [(bx - ax) / 0.8, (by - ay) / 0.8];
    const nudge = (share: number) => [bx + along[0] * share, by + along[1] * share];
    // A little further out is not pulled back to the minimum overlap.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: nudge(0.02)[0], clientY: nudge(0.02)[1] });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: nudge(0.02)[0], clientY: nudge(0.02)[1] });
    expect(Math.abs(middle('p2')[0] - bx)).toBeGreaterThan(1);
    // Well past it stays put, flagged, with no Alt.
    const [cx, cy] = middle('p2');
    fireEvent.pointerDown(stage, pointer(cx, cy));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1] });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: nudge(0.12)[0], clientY: nudge(0.12)[1] });
    expect(screen.queryByText(/Not placed/)).not.toBeInTheDocument();
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'invalid');
    expect(screen.getByTestId('framing-issues')).toHaveTextContent(/r1c1 and p2 overlap by only \d+%, under the 20% minimum/);
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].layout.panels[1].x).toBeLessThan(-0.85);
  });

  it('nudges, locks, removes and undoes the chosen panel from the keyboard', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /p2/ }));
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-selected', 'true');
    expect(screen.getAllByTestId('framing-add-handle')).toHaveLength(8);
    const stage = stageOf();
    const before = middle('p2');
    fireEvent.keyDown(stage, { key: 'ArrowUp', shiftKey: true });
    expect(middle('p2')).not.toEqual(before);
    fireEvent.keyDown(stage, { key: 'z', ctrlKey: true });
    expect(middle('p2')[0]).toBeCloseTo(before[0], 3);
    fireEvent.keyDown(stage, { key: 'l', code: 'KeyL' });
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'locked');
    fireEvent.keyDown(stage, { key: 'Delete' });
    expect(screen.getByText('p2 is locked: unlock it to remove it.')).toBeInTheDocument();
    fireEvent.keyDown(stage, { key: 'l', code: 'KeyL' });
    fireEvent.keyDown(stage, { key: 'Delete' });
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    fireEvent.keyDown(stage, { key: 'z', ctrlKey: true });
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2);
    // A + on the chosen panel's side adds one there.
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    const handle = screen.getAllByTestId('framing-add-handle')[1].getAttribute('transform')!.match(/translate\(([-\d.]+) ([-\d.]+)\)/)!.slice(1).map(Number);
    fireEvent.pointerDown(stage, pointer(handle[0], handle[1]));
    fireEvent.pointerUp(stage, pointer(handle[0], handle[1]));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(3);
    expect(screen.getByText('Added p3 beside r1c1.')).toBeInTheDocument();
  });

  /** The chosen panel's turn handle on the stage. */
  const turnHandleAt = (): [number, number] => {
    const circle = screen.getByTestId('framing-turn-handle').querySelector('circle')!;
    return [Number(circle.getAttribute('cx')), Number(circle.getAttribute('cy'))];
  };
  /** A point a quarter turn round `around`, at the same distance as `from`, by `degrees` on screen. */
  const swung = (around: [number, number], from: [number, number], degrees: number): [number, number] => {
    const rad = (degrees * Math.PI) / 180;
    const dx = from[0] - around[0]; const dy = from[1] - around[1];
    return [around[0] + dx * Math.cos(rad) - dy * Math.sin(rad), around[1] + dx * Math.sin(rad) + dy * Math.cos(rad)];
  };

  it('turns the chosen panel by its handle, springs a turn back that leaves the rule short, and turns it anyway with Alt', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.75, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /p2/ }));
    // A locked panel has no turn handle; the chosen unlocked one has.
    const stage = stageOf();
    const centre = middle('p2');
    const handle = turnHandleAt();
    // A small turn keeps the overlap: the strip and its share follow it live.
    fireEvent.pointerDown(stage, pointer(handle[0], handle[1]));
    const small = swung(centre, handle, -4);
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: small[0], clientY: small[1] });
    expect(screen.getByTestId('framing-overlap')).not.toHaveAttribute('data-short');
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: small[0], clientY: small[1] });
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent(/Turned p2 [+−]\d+\.\d° from the framing/);
    expect(screen.getByLabelText('p2 turn degrees')).not.toHaveValue(0);
    // Near the framing angle it settles on it.
    fireEvent.pointerDown(stage, pointer(turnHandleAt()[0], turnHandleAt()[1]));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: handle[0] + 0.5, clientY: handle[1] });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: handle[0] + 0.5, clientY: handle[1] });
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(0);
    // Far round it leaves the panels short of the minimum: it springs back.
    fireEvent.pointerDown(stage, pointer(handle[0], handle[1]));
    const far = swung(centre, handle, -28);
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: far[0], clientY: far[1] });
    expect(screen.getByTestId('framing-overlap')).toHaveAttribute('data-short', 'true');
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: far[0], clientY: far[1] });
    expect(screen.getByText(/Not turned: r1c1 and p2 overlap by only/)).toBeInTheDocument();
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(0);
    // Alt turns it anyway, flagged.
    fireEvent.pointerDown(stage, pointer(handle[0], handle[1]));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: far[0], clientY: far[1], altKey: true });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: far[0], clientY: far[1], altKey: true });
    expect(screen.getByText(/Turned p2 off the rule/)).toBeInTheDocument();
    const turn = Number((screen.getByLabelText('p2 turn degrees') as HTMLInputElement).value);
    expect(Math.abs(turn)).toBeGreaterThan(20);
    // Undo takes the turn back; the panel editor puts it back to the framing angle too.
    fireEvent.click(screen.getByRole('button', { name: 'Undo panel change' }));
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(0);
    fireEvent.click(screen.getByRole('button', { name: 'Redo panel change' }));
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(Number(turn.toFixed(2)));
    fireEvent.click(screen.getByRole('button', { name: 'Framing angle' }));
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(0);
    fireEvent.change(screen.getByLabelText('p2 turn degrees'), { target: { value: '3' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].layout.panels[1]).toEqual({ id: 'p2', x: -0.75, y: 0, locked: false, angle_offset_degrees: 3 });
    expect(saves[0].layout.panels[0]).toEqual({ id: 'r1c1', x: 0, y: 0, locked: true });
  });

  it('turns the chosen panel with [ and ], refuses a locked one, and sets its angle whatever turns the camera', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.75, y: 0, locked: false }]));
    // RedCat turns its camera by hand; planning sets the angle all the same.
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ ...rigA, profile: { optics: { value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 250, aperture_mm: 51, rotation: { mode: 'manual', angle_degrees: 0 } } } } }, rigB]))));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    const stage = stageOf();
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    expect(screen.queryByTestId('framing-turn-handle')).not.toBeInTheDocument();
    fireEvent.keyDown(stage, { key: ']', code: 'BracketRight' });
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent('r1c1 is locked: it keeps the angle it was shot at.');
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /p2/ }));
    expect(screen.getByTestId('framing-turn-handle')).toBeInTheDocument();
    fireEvent.keyDown(stage, { key: ']', code: 'BracketRight' });
    fireEvent.keyDown(stage, { key: '}', code: 'BracketRight', shiftKey: true });
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(11);
    fireEvent.keyDown(stage, { key: '[', code: 'BracketLeft' });
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(10);
    // Each panel says its own camera angle, and nothing about how the rig reaches it.
    expect(screen.getByTestId('framing-chosen-angle')).toHaveTextContent(/Camera angle here \d+\.\d° east of north, turned \+10\.0° from the framing\./);
    expect(screen.queryByText(/by hand/)).not.toBeInTheDocument();
    fireEvent.keyDown(stage, { key: 'z', ctrlKey: true });
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(11);
  });

  it('draws a shot panel where its frames say, with no note on the sky, and places new panels against its real edges', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    // Twelve solved frames say the framed target was shot with a wider camera, turned 1.5°.
    const shotGeometry = { center: { ra_degrees: seed.center.ra_degrees + 0.05, dec_degrees: seed.center.dec_degrees + 0.02 }, position_angle_degrees: 36.5, size: { width_degrees: 6.2, height_degrees: 4.1 }, source: 'solved' as const, size_source: 'solved' as const, frames: 12 };
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, panels: [mosaic.panels[0]], measured: [{ framing_rig_id: null, panel_id: 'r1c1', geometry: shotGeometry }] }))));
    mount();
    await waitFor(() => expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-shot', 'solved'));
    // The sky says nothing of where the outline comes from, and a lone panel carries no name.
    expect(screen.queryByTestId('framing-panel-shot')).toBeNull();
    expect(document.querySelector("[data-panel-id='r1c1'] text")).toBeNull();
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    expect(screen.getByTestId('framing-chosen-shot')).toHaveTextContent('Drawn as shot: 6.20° × 4.10° at 36.5°.');
    // A column east of it overlaps the wider shot panel by the minimum of its side, not of the plan's.
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    expect(await screen.findByText(/Added p2 on the E side/)).toBeInTheDocument();
    expect(screen.queryByTestId('framing-issues')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    // Saved quietly with the shot geometry the server read, locked.
    expect(saves[0].layout.panels[0]).toEqual({ id: 'r1c1', x: 0, y: 0, locked: true, measured: shotGeometry });
    expect(saves[0].layout.panels[1].x).toBeLessThan(-0.8);
  });

  it('unlocks a panel with frames by hand, and leaves it unlocked', async () => {
    const shotGeometry = { center: { ra_degrees: seed.center.ra_degrees + 0.05, dec_degrees: seed.center.dec_degrees + 0.02 }, position_angle_degrees: 36.5, size: { width_degrees: 6.2, height_degrees: 4.1 }, source: 'header' as const, size_source: 'header' as const, frames: 8 };
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, panels: [mosaic.panels[0]], measured: [{ framing_rig_id: null, panel_id: 'r1c1', geometry: shotGeometry }] }))));
    mount();
    await waitFor(() => expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-panel-state', 'locked'));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Unlock' }));
    expect(document.querySelector("[data-panel-id='r1c1']")).not.toHaveAttribute('data-panel-state');
    expect(screen.getByRole('button', { name: 'Lock' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].layout.panels[0]).toEqual({ id: 'r1c1', x: 0, y: 0, locked: false, unlocked_by_hand: true });
  });

  it('does not lock again a panel with frames that was unlocked by hand', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false, unlocked_by_hand: true }]));
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, panels: [mosaic.panels[0]] }))));
    mount();
    // Its frames are read, and still it is not locked.
    await waitFor(() => expect(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ })).toHaveTextContent(/frames?/));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    expect(screen.getByRole('button', { name: 'Lock' })).toBeInTheDocument();
    expect(document.querySelector("[data-panel-id='r1c1']")).not.toHaveAttribute('data-panel-state');
    // Laid out again as a grid, it stays unlocked and moves with the grid.
    fireEvent.change(screen.getByLabelText('Mosaic columns'), { target: { value: '2' } });
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    expect(document.querySelector("[data-panel-id='r1c1']")).not.toHaveAttribute('data-panel-state');
    expect(screen.queryByText(/went back to the saved place/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].layout.panels[0]).toMatchObject({ id: 'r1c1', locked: false, unlocked_by_hand: true });
    expect(saves[0].layout.panels[1].unlocked_by_hand).toBeUndefined();
  });

  it('gives a rig framed on its own the panel tools, and keeps its locks', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    expect(screen.getByRole('group', { name: 'Mosaic panels · C925 data' })).toBeInTheDocument();
    // Its own panels grow, are chosen, turned and undone with the shared framing's tools.
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    expect(await screen.findByText(/Added p2 on the E side/)).toBeInTheDocument();
    expect(screen.getByTestId('framing-panels').querySelectorAll('polygon')).toHaveLength(2);
    expect(screen.getByTestId('framing-shared-panels').querySelectorAll('polygon')).toHaveLength(1);
    const stage = stageOf();
    fireEvent.keyDown(stage, { key: ']', code: 'BracketRight', shiftKey: true });
    expect(screen.getByLabelText('p2 turn degrees')).toHaveValue(10);
    fireEvent.keyDown(stage, { key: 'l', code: 'KeyL' });
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'locked');
    // Its locked panel holds its center and angle.
    expect(screen.getByLabelText('C925 data right ascension degrees')).toBeDisabled();
    expect(screen.getByLabelText('C925 data camera angle degrees')).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    const own = saves[0].rig_framings![0];
    expect(own.layout.panels.map(panel => [panel.id, panel.locked, panel.angle_offset_degrees ?? 0])).toEqual([['r1c1', false, 0], ['p2', true, 10]]);
    // The shared framing is as it was.
    expect(saves[0].layout.panels).toEqual([{ id: 'r1c1', x: 0, y: 0, locked: false }]);
  });
  it('asks before another rig\'s field moves a shared mosaic, then frames that rig separately or uses its field', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107',
      field_of_view: { width_degrees: 1.2, height_degrees: 0.8, pixel_scale_arcsec: 1.9, focal_ratio: 7 } };
    // p2 was unlocked by hand on the shared framing; the separate copy keeps that.
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }, { id: 'p2', x: -0.8, y: 0, locked: false, unlocked_by_hand: true }]));
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    const source = () => screen.getByTestId('framing-panel-source');
    const askar = () => screen.getByRole('group', { name: 'Askar 107' });
    // Its card says the shared panels are not sized for it.
    expect(askar()).toHaveTextContent('Shared framing · 2 panels · field differs');
    // Nothing moves yet: Askar's card asks, and the question takes the focus.
    fireEvent.click(screen.getByRole('button', { name: 'Frame with Askar 107' }));
    const question = within(askar()).getByRole('group', { name: 'Its field moves all 2 panels.' });
    expect(question).toHaveFocus();
    expect(source()).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°');
    const ask = () => within(screen.getByTestId('framing-rig-ask'));
    fireEvent.click(ask().getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('framing-rig-ask')).not.toBeInTheDocument();
    // The focus goes back to the rig's card.
    expect(askar()).toHaveFocus();
    // Framed separately, Askar gets its own copy and the shared mosaic keeps its size.
    fireEvent.click(screen.getByRole('button', { name: 'Frame with Askar 107' }));
    fireEvent.click(ask().getByRole('button', { name: 'Frame separately' }));
    expect(screen.getByRole('group', { name: 'Mosaic panels · Askar 107' })).toBeInTheDocument();
    expect(source()).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°');
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].panel).toEqual({ width_degrees: 5.38, height_degrees: 3.6 });
    expect(saves[0].rig_framings!.map(own => [own.rig_id, own.layout.panels.map(panel => [panel.id, panel.unlocked_by_hand ?? false])])).toEqual([[rigC.rig.id, [['r1c1', false], ['p2', true]]]]);
    // Back on the shared framing, Use its field sizes the shared mosaic by Askar.
    fireEvent.click(within(askar()).getByRole('button', { name: 'Use shared' }));
    fireEvent.click(screen.getByRole('button', { name: 'Frame with Askar 107' }));
    fireEvent.click(ask().getByRole('button', { name: 'Use its field' }));
    expect(source()).toHaveTextContent('Size from Askar 107 · 1.20° × 48.0′');
    expect(screen.queryByTestId('framing-rig-ask')).not.toBeInTheDocument();
  });

  it('drops the question when its rig goes off, and hands the size to a rig with the same field', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107',
      field_of_view: { width_degrees: 1.2, height_degrees: 0.8, pixel_scale_arcsec: 1.9, focal_ratio: 7 } };
    const rigD = { ...rigA, rig: { ...rigA.rig, id: 'dddddddd-dddd-4ddd-8ddd-dddddddddddd', name: 'Twin' }, catalog_slug: 'twin', catalog_name: 'Twin RedCat' };
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC, rigD]))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const view = (shooting: string[]) => <QueryClientProvider client={client}><FramingView projectId="project" seed={seed} preferredRigIds={[rigA.rig.id]}
      shootingRigIds={shooting} onToggleRig={() => {}} /></QueryClientProvider>;
    const { rerender } = render(view([rigA.rig.id, rigD.rig.id]));
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°'));
    // Turned on, Askar asks once the page has heard it is on.
    fireEvent.click(screen.getByRole('switch', { name: 'Askar 107 on' }));
    rerender(view([rigA.rig.id, rigC.rig.id, rigD.rig.id]));
    expect(await screen.findByTestId('framing-rig-ask')).toHaveFocus();
    expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°');
    rerender(view([rigA.rig.id, rigD.rig.id]));
    await waitFor(() => expect(screen.queryByTestId('framing-rig-ask')).not.toBeInTheDocument());
    // Askar is on again; RedCat goes off. Twin RedCat has the same field, so it takes over, though Askar comes first.
    rerender(view([rigC.rig.id, rigD.rig.id]));
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from Twin RedCat · 5.38° × 3.60°'));
    expect(screen.queryByTestId('framing-rig-ask')).not.toBeInTheDocument();
  });

  it('keeps a shared mosaic\'s size when its rig goes off and the rig that is on has another field', async () => {
    const rigC = { ...rigA, rig: { ...rigA.rig, id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Askar' }, catalog_slug: 'askar', catalog_name: 'Askar 107',
      field_of_view: { width_degrees: 1.2, height_degrees: 0.8, pixel_scale_arcsec: 1.9, focal_ratio: 7 } };
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    server.use(http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([rigA, rigB, rigC]))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const view = (shooting: string[]) => <QueryClientProvider client={client}><FramingView projectId="project" seed={seed} preferredRigIds={[rigA.rig.id]}
      shootingRigIds={shooting} onToggleRig={() => {}} /></QueryClientProvider>;
    const { rerender } = render(view([rigA.rig.id, rigC.rig.id]));
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 · 5.38° × 3.60°'));
    rerender(view([rigC.rig.id]));
    await waitFor(() => expect(screen.getByTestId('framing-panel-source')).toHaveTextContent('Size from RedCat 61 (off) · 5.38° × 3.60°'));
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('2 panels');
  });

  it('reads a separate framing\'s gaps over every framing on the sky', async () => {
    // C925 framed on its own: two half-size panels half a panel apart, either
    // side of the shared framing's one panel, which fills the sky between.
    const draft = { ...placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]), rig_framings: [{ rig_id: rigB.rig.id, center: null, position_angle_degrees: null,
      layout: { min_overlap_percent: 20, panels: [{ id: 'r1c1', x: -0.75, y: 0, locked: false }, { id: 'p2', x: 0.75, y: 0, locked: false }] },
      panel: { width_degrees: 2.69, height_degrees: 1.8 } }] };
    fixture(draft);
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Edit' }));
    expect(screen.getByRole('group', { name: 'Mosaic panels · C925 data' })).toBeInTheDocument();
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2);
    expect(screen.queryByTestId('framing-issues')).not.toBeInTheDocument();
    expect(document.querySelector("[data-panel-id='p2']")).not.toHaveAttribute('data-panel-state');
    // Moved off the shared panel, its own gap is said.
    fireEvent.change(screen.getByLabelText('C925 data right ascension degrees'), { target: { value: '40' } });
    expect(screen.getByTestId('framing-issues')).toHaveTextContent('r1c1 and p2 leave a gap of 50% of a panel.');
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'invalid');
  });

  it('says a lock or unlock on a grid panel is unsaved on the plan page', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    const drafts = mountOnPage();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Lock' }));
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    expect(drafts.current!.sections[0].changes).toEqual(['r1c1 locked']);
  });

  it('reads the panels again after a save, after a refused one, and on Reload', async () => {
    let mosaicGets = 0;
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }]));
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => { mosaicGets += 1; return HttpResponse.json(ok(unshot)); }));
    mount();
    await waitFor(() => expect(mosaicGets).toBe(1));
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(mosaicGets).toBe(2));
    server.use(http.put('/api/director/v1/projects/project/framing', () => HttpResponse.json({ success: false, data: null, error: 'Locked panel r1c1 would move.' }, { status: 422 })));
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(mosaicGets).toBe(3));
    expect(await screen.findByText(/Locked panel r1c1 would move/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Reload framing' }));
    await waitFor(() => expect(mosaicGets).toBe(4));
  });

  it('holds the shared center and angle for a rig framed on its own that follows them with locked panels', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    expect(await screen.findByText(/Added p2 on the E side/)).toBeInTheDocument();
    expect(screen.getByLabelText('Right ascension degrees')).toBeEnabled();
    fireEvent.keyDown(stageOf(), { key: 'l', code: 'KeyL' });
    expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'locked');
    // Its locked panels sit on the shared center and angle, so they hold them.
    expect(screen.getByLabelText('Right ascension degrees')).toBeDisabled();
    expect(screen.getByLabelText('Position angle degrees')).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'Find a target' })).toBeDisabled();
    expect(screen.getByTestId('framing-held')).toHaveTextContent('Locked panels of C925 data follow the shared center and camera angle');
    // A locked planned panel off its center holds its panel size.
    expect(screen.getByLabelText('C925 data panel width degrees')).toBeDisabled();
    // Taking a center of its own where it is moves nothing, so it may; going back to the shared one may not.
    const follows = screen.getByLabelText('C925 data follows the shared center');
    expect(follows).toBeEnabled();
    fireEvent.click(follows);
    expect(follows).not.toBeChecked();
    expect(follows).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Move to view center' })).toBeDisabled();
    expect(screen.getByLabelText('Right ascension degrees')).toBeEnabled();
    expect(screen.getByLabelText('Position angle degrees')).toBeDisabled();
    expect(screen.getByLabelText('C925 data follows the shared angle')).toBeEnabled();
  });

  it('ignores a second pointer during a drag, offers to place a refused drop anyway, and puts the panel back on a cancel, Escape or a lost release', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    fireEvent.click(screen.getByRole('button', { name: 'Drag moves one panel' }));
    const stage = stageOf();
    const [ax, ay] = middle('r1c1');
    const [bx, by] = middle('p2');
    const along = [(bx - ax) / 0.8, (by - ay) / 0.8];
    const far = [bx + along[0] * 0.12, by + along[1] * 0.12];
    // A second finger lands and lifts mid-drag: the drag carries on and is checked.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: far[0], clientY: far[1] });
    fireEvent.pointerDown(stage, { button: 0, pointerId: 2, clientX: 900, clientY: 700 });
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 2, clientX: 950, clientY: 720 });
    fireEvent.pointerUp(stage, { pointerId: 2, clientX: 950, clientY: 720 });
    expect(middle('p2')[0]).toBeCloseTo(far[0], 0);
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: far[0], clientY: far[1] });
    expect(screen.getByText(/Not placed: r1c1 and p2 overlap by only 8%/)).toBeInTheDocument();
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    // The notice offers to place it anyway, as one step to undo.
    fireEvent.click(screen.getByRole('button', { name: 'Place anyway' }));
    expect(screen.getByText(/Placed p2 off the rule: r1c1 and p2 overlap by only 8%/)).toBeInTheDocument();
    expect(middle('p2')[0]).toBeCloseTo(far[0], 0);
    fireEvent.click(screen.getByRole('button', { name: 'Undo panel change' }));
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    // A cancelled touch puts it back unchecked.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: far[0], clientY: far[1] });
    fireEvent.pointerCancel(stage, { pointerId: 1 });
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent('Put p2 back.');
    // So does Escape.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: far[0], clientY: far[1] });
    fireEvent.keyDown(stage, { key: 'Escape' });
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: far[0], clientY: far[1] });
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    // A move with no button held means the release was lost: the drag ends there.
    fireEvent.pointerDown(stage, pointer(bx, by));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: bx + 5, clientY: by });
    fireEvent.pointerMove(stage, { buttons: 0, pointerId: 1, clientX: bx + 200, clientY: by + 100 });
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: bx + 200, clientY: by + 100 });
    expect(middle('p2')[0]).toBeCloseTo(bx, 1);
    expect(screen.getByRole('button', { name: 'Undo panel change' })).toBeDisabled();
  });

  it('starts a fresh panel history after Discard and Back to saved framing', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    const drafts = mountOnPage();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    fireEvent.click(screen.getByRole('button', { name: "Add a row on the mosaic's north side" }));
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    drafts.current!.discardAll();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    expect(screen.getByRole('button', { name: 'Undo panel change' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    fireEvent.click(screen.getByRole('button', { name: 'Back to saved framing' }));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Undo panel change' })).toBeDisabled();
  });

  it('leaves keys pressed on a button over the sky to that button, and says why the last panel stays', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    // The notice is there before it has words, so its first are heard.
    expect(screen.getByTestId('framing-mosaic-notice')).toBeEmptyDOMElement();
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /p2/ }));
    const zoom = screen.getByRole('button', { name: 'Zoom in' });
    fireEvent.keyDown(zoom, { key: 'Delete' });
    fireEvent.keyDown(zoom, { key: 'l', code: 'KeyL' });
    fireEvent.keyDown(zoom, { key: 'ArrowLeft' });
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2);
    expect(document.querySelector("[data-panel-id='p2']")).not.toHaveAttribute('data-panel-state', 'locked');
    const stage = stageOf();
    fireEvent.keyDown(stage, { key: 'Delete' });
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    fireEvent.keyDown(stage, { key: 'Delete' });
    const notice = screen.getByTestId('framing-mosaic-notice');
    const first = within(notice).getByText('r1c1 is the only panel: a framing keeps one.');
    // The same words said again are a new announcement.
    fireEvent.keyDown(stage, { key: 'Delete' });
    expect(within(notice).getByText('r1c1 is the only panel: a framing keeps one.')).not.toBe(first);
    // With the panel let go, adding beside it is not offered as chosen.
    fireEvent.click(screen.getByRole('button', { name: 'Beside r1c1' }));
    expect(screen.getByRole('button', { name: 'Beside r1c1' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    expect(screen.getByRole('button', { name: 'Beside a panel' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: 'Whole mosaic' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('puts a panel moved before its frames were known back where it was saved, locked', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }, { id: 'p2', x: -0.8, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(2));
    const saved = middle('p2');
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /p2/ }));
    fireEvent.keyDown(stageOf(), { key: 'ArrowUp', shiftKey: true });
    expect(Math.hypot(middle('p2')[0] - saved[0], middle('p2')[1] - saved[1])).toBeGreaterThan(5);
    // Frames for p2 turn up when the panels are read again.
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...unshot, panels: [unshot.panels[0], { ...unshot.panels[1], panel_id: 'p2', progress: { desired: 72, acquired: 6, accepted: 6 } }] }))));
    fireEvent.click(screen.getByRole('button', { name: 'Reload framing' }));
    await waitFor(() => expect(document.querySelector("[data-panel-id='p2']")).toHaveAttribute('data-panel-state', 'locked'));
    expect(middle('p2')[0]).toBeCloseTo(saved[0], 1);
    expect(middle('p2')[1]).toBeCloseTo(saved[1], 1);
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent('p2 has frames, so it went back to the saved place and is locked.');
  });

  it('keeps frames on the framing they were shot under until a separate framing is saved', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    server.use(http.get('/api/director/v1/projects/project/mosaic', () => HttpResponse.json(ok({ ...mosaic, panels: [mosaic.panels[0]] }))));
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-panel-state', 'locked'));
    fireEvent.click(within(screen.getByRole('group', { name: 'RedCat 61' })).getByRole('button', { name: 'Frame separately' }));
    // Its own panels are a new plan: nothing was shot under them yet.
    expect(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ })).not.toHaveTextContent('40 frames');
    // The shared panel keeps its frames and its lock.
    fireEvent.click(within(screen.getByRole('group', { name: 'RedCat 61' })).getByRole('button', { name: 'Done' }));
    expect(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ })).toHaveTextContent('40 frames');
    expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-panel-state', 'locked');
  });

  it('takes a press on a handle within a fingertip on a phone-sized stage', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    mount();
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('list', { name: 'Panels' })).getByRole('button', { name: /r1c1/ }));
    const stage = screen.getByTestId('framing-stage');
    stage.setPointerCapture = vi.fn();
    stage.getBoundingClientRect = () => ({ left: 0, top: 0, width: 390, height: 292, right: 390, bottom: 292, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperty(stage, 'clientWidth', { value: 390, configurable: true });
    // The west side's handle, in stage units, and 40 units (15 CSS pixels) below it.
    const [hx, hy] = screen.getAllByTestId('framing-add-handle')[3].getAttribute('transform')!.match(/translate\(([-\d.]+) ([-\d.]+)\)/)!.slice(1).map(Number);
    const k = 1024 / 390;
    fireEvent.pointerDown(stage, pointer(hx / k, (hy + 40) / k));
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: hx / k, clientY: (hy + 40) / k });
    expect(await screen.findByText(/Added p2 beside r1c1/)).toBeInTheDocument();
  });

  it('looks around instead of moving a held center when the sky is dragged', async () => {
    const { saves } = fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: true }]));
    mount();
    await waitFor(() => expect(document.querySelector("[data-panel-id='r1c1']")).toHaveAttribute('data-panel-state', 'locked'));
    fireEvent.click(screen.getByRole('button', { name: 'Drag moves the sky' }));
    const stage = stageOf();
    const ra = (screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value;
    const view = screen.getByTestId('framing-view-center').textContent;
    fireEvent.pointerDown(stage, pointer(100, 100));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 300, clientY: 200 });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: 300, clientY: 200 });
    expect((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value).toBe(ra);
    expect(screen.getByTestId('framing-view-center').textContent).not.toBe(view);
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent('Locked panels hold the mosaic where it is');
    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].center).toEqual(seed.center);
  });

  it('holds the shared center against a sky drag for a rig framed on its own that follows it', async () => {
    fixture(placed([{ id: 'r1c1', x: 0, y: 0, locked: false }]));
    mount(true, true, [rigA.rig.id]);
    await waitFor(() => expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1));
    fireEvent.click(within(screen.getByRole('group', { name: 'C925 data' })).getByRole('button', { name: 'Frame separately' }));
    fireEvent.click(screen.getByRole('button', { name: "Add a column on the mosaic's east side" }));
    expect(await screen.findByText(/Added p2 on the E side/)).toBeInTheDocument();
    const stage = stageOf();
    fireEvent.keyDown(stage, { key: 'l', code: 'KeyL' });
    fireEvent.click(screen.getByRole('button', { name: 'Drag moves the sky' }));
    const ra = (screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value;
    fireEvent.pointerDown(stage, pointer(100, 100));
    fireEvent.pointerMove(stage, { buttons: 1, pointerId: 1, clientX: 300, clientY: 200 });
    fireEvent.pointerUp(stage, { pointerId: 1, clientX: 300, clientY: 200 });
    expect((screen.getByLabelText('Right ascension degrees') as HTMLInputElement).value).toBe(ra);
    expect(screen.getByTestId('framing-mosaic-notice')).toHaveTextContent('Locked panels of C925 data hold the mosaic where it is');
  });
});
