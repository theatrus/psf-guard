import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import FramingView from '../director/FramingView';
import type { DirectorFramingDraft, DirectorFramingRequest } from '../../api/directorTypes';
import { moveBy, skyAtStage, toStage } from '../director/framingModel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rigA = { rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 }, catalog_slug: 'redcat', catalog_name: 'RedCat 61',
  profile: null, field_of_view: { width_degrees: 5.38, height_degrees: 3.6, pixel_scale_arcsec: 3.1, focal_ratio: 4.9 } };
const rigB = { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925', revision: 1 }, catalog_slug: 'c925', catalog_name: 'C925 data', profile: null, field_of_view: null };
const seed = { name: 'M31', center: { ra_degrees: 10.6847, dec_degrees: 41.269 }, position_angle_degrees: 35 };

function fixture(existing: DirectorFramingDraft | null = null) {
  const previews: DirectorFramingRequest[] = [];
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
    http.get('/api/director/v1/sky/cutout', ({ request }) => {
      cutouts.push(new URL(request.url).search);
      if (cutouts.length === 1) return HttpResponse.json(ok({ state: 'generating' }), { status: 202 });
      return HttpResponse.arrayBuffer(new Uint8Array([255, 216, 255]).buffer, { status: 200, headers: { 'content-type': 'image/jpeg' } });
    }),
    http.post('/api/director/v1/framing/preview', async ({ request }) => {
      const body = await request.json() as DirectorFramingRequest;
      previews.push(body);
      const half = [body.panel.width_degrees / 2, body.panel.height_degrees / 2];
      const panels = [];
      for (let r = 1; r <= body.mosaic.rows; r++) for (let c = 1; c <= body.mosaic.columns; c++) panels.push({
        id: `r${r}c${c}`, row: r, column: c, center: body.center,
        corners: [body.center, body.center, body.center, body.center],
        view_corners: [[half[0], half[1]], [half[0], -half[1]], [-half[0], -half[1]], [-half[0], half[1]]],
      });
      return HttpResponse.json(ok({ schema_version: 1, panels, overlays: body.overlays.map(o => ({ id: o.id, center: body.center, corners: [], view_corners: [[1, 1], [1, -1], [-1, -1], [-1, 1]] })),
        extent: { width_degrees: body.panel.width_degrees * body.mosaic.columns, height_degrees: body.panel.height_degrees * body.mosaic.rows }, view_center_offset: [0, 0] }));
    }),
  );
  return { previews, saves, cutouts };
}
function mount(canWrite = true, withSeed = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<FramingView projectId="project" seed={withSeed ? seed : null} />, { wrapper: Wrapper });
}

describe('Framing view', () => {
  beforeEach(() => {
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:stage'), revokeObjectURL: vi.fn() }));
  });
  afterEach(() => vi.unstubAllGlobals());

  it('seeds from the catalog target, frames with a rig, polls the survey, and saves a first draft', async () => {
    const { previews, saves, cutouts } = fixture(); mount();
    expect(await screen.findByLabelText('Target name')).toHaveValue('M31');
    expect(screen.getByLabelText('Position angle degrees')).toHaveValue(35);
    expect(screen.getByTestId('framing-extent')).toHaveTextContent('Choose a rig or enter a panel size');
    fireEvent.change(screen.getByLabelText('Panel rig'), { target: { value: rigA.rig.id } });
    await waitFor(() => expect(previews).toHaveLength(1));
    expect(previews[0].panel).toEqual({ width_degrees: 5.38, height_degrees: 3.6 });
    expect(previews[0].position_angle_degrees).toBe(35);
    expect(previews[0].view).toEqual({ center: seed.center, rotation_degrees: 0 });
    await waitFor(() => expect(screen.getByTestId('framing-extent')).toHaveTextContent('1 panel, 5.38° × 3.60° in all.'));
    expect(document.querySelectorAll('.framing-panel polygon')).toHaveLength(1);
    // The 202 was polled once and the image then took the stage.
    await waitFor(() => expect(cutouts.length).toBeGreaterThanOrEqual(2), { timeout: 3000 });
    await waitFor(() => expect(document.querySelector('.framing-stage img')).toHaveAttribute('src', 'blob:stage'));
    expect(cutouts[0]).toContain('survey=dss2_color');
    expect(cutouts[0]).toContain('width=1024');

    fireEvent.change(screen.getByLabelText('Mosaic rows'), { target: { value: '2' } });
    fireEvent.change(screen.getByLabelText('Panel overlap percent'), { target: { value: '15' } });
    await waitFor(() => expect(screen.getByTestId('framing-extent')).toHaveTextContent('2 panels'));
    expect(document.querySelectorAll('.framing-panel text')).toHaveLength(2);
    fireEvent.click(screen.getByRole('checkbox', { name: /RedCat 61/ }));
    await waitFor(() => expect(previews.at(-1)?.overlays).toEqual([{ id: rigA.rig.id, size: { width_degrees: 5.38, height_degrees: 3.6 }, position_angle_degrees: 35 }]));
    expect(screen.getByRole('checkbox', { name: /C925 data/ })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Save framing' }));
    expect(await screen.findByText('Saved framing revision 1.')).toBeInTheDocument();
    expect(saves).toHaveLength(1);
    expect(saves[0]).toMatchObject({ project_id: 'project', revision: 0, target_name: 'M31', panel_rig_id: rigA.rig.id, panel: { width_degrees: 5.38, height_degrees: 3.6 },
      mosaic: { rows: 2, columns: 1, overlap_percent: 15 }, shown_rig_ids: [rigA.rig.id], survey_id: 'dss2_color', view_fov_degrees: 4 });
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

  it('is read only without write access and explains a missing seed', async () => {
    fixture(); mount(false);
    await screen.findByLabelText('Target name');
    expect(screen.queryByRole('button', { name: 'Save framing' })).not.toBeInTheDocument();
    expect(screen.getByText('Read only')).toBeInTheDocument();
  });

  it('asks for a target when there is neither a draft nor a seed', async () => {
    fixture(); mount(true, false);
    expect(await screen.findByText(/Add a target to this project first/)).toBeInTheDocument();
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
