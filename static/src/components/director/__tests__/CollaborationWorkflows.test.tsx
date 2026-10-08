import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import CollaborationWorkflows from '../CollaborationWorkflows';
import type { CollaborationConnection } from '../../../api/collaborationTypes';

const connection: CollaborationConnection = {
  status: 'registered', binding: { id: 'connection', rig_id: 'rig', name: 'Rig', base_url: 'https://collab.example/', agent_id: '000000000001', allow_loopback_http: false, state: 'registered',
    settings: { binning: 1, colour: false, hours_per_night: 6, share_status: false, filters: { Ha: { exposure_seconds: 300, bandpass_nm: 7 } } } },
};
const share = { task_id: '000000000004', name: 'M31', version: 2, review_reasons: [], demands: [{ panel_index: 0, filter: 'H', exposure_ms: 300000, requested_frames: 12 }] };
function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<MemoryRouter><QueryClientProvider client={client}><CollaborationWorkflows connection={connection} canWrite refresh={() => undefined} /></QueryClientProvider></MemoryRouter>);
  return client;
}
describe('Collaboration work transfer', () => {
  it('makes a newly joined project immediately available to automation without another browse', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'browse'
        ? { projects: [{ project_id: '000000000002', name: 'M31', joined: false, compatible: true }] }
        : body.operation === 'background_status'
          ? { policy: null, catalogs: [], projects: [], status: { running: false, last_started_ms: null, last_success_ms: null, next_run_ms: null, last_error: null, result: null } }
          : { night: { night: '2026-10-05', moon: 0.12, moon_up: 0.3 }, shares: [share] };
      return HttpResponse.json({ success: true, data });
    }));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    try {
      setup();
      await userEvent.click(screen.getByRole('button', { name: 'Browse projects' }));
      await userEvent.click(await screen.findByRole('button', { name: 'Join' }));
      expect(await screen.findByRole('button', { name: 'Joined' })).toBeDisabled();
      await userEvent.click(screen.getByRole('tab', { name: 'Automation' }));
      expect(await screen.findByLabelText('M31')).not.toBeChecked();
      expect(requests.filter(r => r.operation === 'browse')).toHaveLength(1);
      expect(requests.find(r => r.operation === 'join')).toEqual({ operation: 'join', project: '000000000002' });
    } finally {
      confirm.mockRestore();
    }
  });
  it('explains that failed check-ins retain queued contributions and permits retry', async () => {
    server.use(http.post('/api/director/v1/collaboration/connection/work', () =>
      HttpResponse.json({ error: 'Collaboration server returned an unsuccessful response' }, { status: 502 })));
    setup();
    await userEvent.click(screen.getByRole('button', { name: 'Check in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Check-in failed; queued reports retained. Collaboration server returned an unsuccessful response');
    expect(screen.getByRole('button', { name: 'Check in' })).toBeEnabled();
  });
  it('pulls tonight without manual context and freezes the resolved night through review and apply', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'tonight' ? { night: { night: '2026-10-05', moon: 0.12, moon_up: 0.3 }, shares: [share] } : body.operation === 'preview'
        ? { preview: { review_digest: 'review-current', action: 'create', acquisition_enabled: false }, plan: { project_id: 'project', night: '2026-10-05', share } }
        : {};
      return HttpResponse.json({ success: true, data });
    }));
    setup();
    expect(screen.getByRole('button', { name: "Pull tonight's work" })).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: "Pull tonight's work" }));
    await waitFor(() => expect(requests[0]).toEqual({ operation: 'tonight' }));
    expect(screen.queryByRole('button', { name: 'Import draft' })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole('button', { name: 'Review import' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Import draft' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'apply', task: share.task_id, night: { night: '2026-10-05', moon: 0.12, moon_up: 0.3 }, review_digest: 'review-current' }));
    expect(await screen.findByText('Imported as an inactive project draft')).toBeVisible();
  });
  it('offers a date-only override and clears a stale import review when it changes', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      return HttpResponse.json({ success: true, data: body.operation === 'tonight'
        ? { night: { night: '2026-10-12', moon: 0.2, moon_up: 0.4 }, shares: [share] }
        : { preview: { review_digest: 'review', action: 'create', acquisition_enabled: false }, plan: { project_id: 'project', night: '2026-10-12', share } } });
    }));
    setup();
    await userEvent.click(screen.getByText('Night override'));
    fireEvent.change(screen.getByLabelText('Observing date'), { target: { value: '2026-10-12' } });
    await userEvent.click(screen.getByRole('button', { name: "Pull tonight's work" }));
    await userEvent.click(await screen.findByRole('button', { name: 'Review import' }));
    await screen.findByRole('button', { name: 'Import draft' });
    expect(requests[0]).toEqual({ operation: 'tonight', observing_date: '2026-10-12' });
    fireEvent.change(screen.getByLabelText('Observing date'), { target: { value: '' } });
    expect(screen.queryByRole('button', { name: 'Import draft' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review import' })).not.toBeInTheDocument();
  });
  it('does not automatically retry a failed report delivery', async () => {
    let calls = 0;
    server.use(http.post('/api/director/v1/collaboration/connection/work', () => {
      calls++; return HttpResponse.json({ success: false, error: 'Server offline; queued reports retained' }, { status: 502 });
    }));
    setup(); await userEvent.click(screen.getByRole('button', { name: 'Check in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('queued reports retained');
    expect(calls).toBe(1);
  });
  it('queues only explicitly selected GUIDs after reviewing unchanged evidence', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'report_inputs' ? { imports: [{ id: 'import', name: 'M31', night: '2026-10-05', panels: [0, 1] }], catalogs: [{ id: 'rig-db', name: 'Rig DB' }] }
        : body.operation === 'report_candidates' ? { images: [{ guid: 'image-guid', file: 'saved.fits', target: 'M31', filter: 'Ha', captured_at: 1791171000 }] }
        : body.operation === 'preview_report' ? { review_digest: 'evidence-digest', report: { frames: 1, seconds: 300, filterName: 'H', calibrated: false, footprint: { width: 1, height: 1 } } } : {};
      return HttpResponse.json({ success: true, data });
    }));
    setup();
    await userEvent.click(screen.getByRole('tab', { name: 'Reports' }));
    await screen.findByRole('option', { name: 'M31 (2026-10-05)' });
    await userEvent.selectOptions(screen.getByLabelText('Imported visit'), 'import');
    expect(screen.queryByRole('combobox', { name: 'Rig database' })).not.toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText('Remote panel'), '0');
    await userEvent.click(await screen.findByLabelText('Select saved.fits'));
    expect(screen.queryByRole('button', { name: 'Queue finalized contribution' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Review 1 images' }));
    await screen.findByRole('button', { name: 'Queue finalized contribution' });
    fireEvent.change(screen.getByLabelText('Observing night'),{target:{value:'2026-10-12'}});
    expect(screen.queryByRole('button', { name: 'Queue finalized contribution' })).not.toBeInTheDocument();
    await waitFor(()=>expect(requests.at(-1)).toEqual({operation:'report_candidates',import_id:'import',catalog:'rig-db',observing_night:'2026-10-12'}));
    await userEvent.click(await screen.findByLabelText('Select saved.fits'));
    await userEvent.click(screen.getByRole('button', { name: 'Review 1 images' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Queue finalized contribution' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'queue_report', selection: { import_id: 'import', catalog: 'rig-db', panel: 0, image_guids: ['image-guid'], observing_night: '2026-10-12' }, review_digest: 'evidence-digest' }));
    expect(await screen.findByText('Contribution queued for check-in')).toBeVisible();
  });
  it('can report an original panel removed from the current assignment', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'report_inputs' ? { imports: [{ id: 'import', name: 'M31', night: '2026-10-05', panels: [0] }], catalogs: [{ id: 'rig-db', name: 'Rig DB' }] }
        : body.operation === 'report_candidates' ? { images: [{ guid: 'old-image', file: 'old-panel.fits', target: 'M31', filter: 'Ha', captured_at: 1791171000, panel: 7, source_digest: 'original-revision' }] }
        : body.operation === 'preview_report' ? { review_digest: 'old-evidence', report: { frames: 1, seconds: 300, filterName: 'H', calibrated: false, footprint: { width: 1, height: 1 } } } : {};
      return HttpResponse.json({ success: true, data });
    }));
    setup();
    await userEvent.click(screen.getByRole('tab', { name: 'Reports' }));
    await screen.findByRole('option', { name: 'M31 (2026-10-05)' });
    await userEvent.selectOptions(screen.getByLabelText('Imported visit'), 'import');
    expect(screen.queryByRole('combobox', { name: 'Rig database' })).not.toBeInTheDocument();
    await screen.findByRole('option', { name: '7' });
    await userEvent.selectOptions(screen.getByLabelText('Remote panel'), '7');
    await userEvent.click(await screen.findByLabelText('Select old-panel.fits'));
    await userEvent.click(screen.getByRole('button', { name: 'Review 1 images' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Queue finalized contribution' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'queue_report', selection: { import_id: 'import', catalog: 'rig-db', panel: 7, image_guids: ['old-image'], observing_night: '2026-10-05', source_digest: 'original-revision' }, review_digest: 'old-evidence' }));
  });
  it('offers arriving accepted frames without selecting them and withdraws a rejected selection', async () => {
    const image = (guid: string) => ({ guid, file: `${guid}.fits`, target: 'M31', filter: 'Ha', captured_at: 1791171000 });
    let images = [image('first')];
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'report_inputs' ? { imports: [{ id: 'import', name: 'M31', night: '2026-10-05', panels: [0] }], catalogs: [{ id: 'rig-db', name: 'Rig DB' }] }
        : body.operation === 'report_candidates' ? { images }
        : body.operation === 'preview_report' ? { review_digest: 'review', report: { frames: 1, seconds: 300, exposure: 300, filterName: 'H', hfr: 2.5, scale: 1.2, focalLength: 300, colour: false, calibrated: false, footprint: { width: 1, height: 1 } } } : {};
      return HttpResponse.json({ success: true, data });
    }));
    const client = setup();
    await userEvent.click(screen.getByRole('tab', { name: 'Reports' }));
    await screen.findByRole('option', { name: 'M31 (2026-10-05)' });
    await userEvent.selectOptions(screen.getByLabelText('Imported visit'), 'import');
    expect(screen.queryByRole('combobox', { name: 'Rig database' })).not.toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText('Remote panel'), '0');
    await userEvent.click(await screen.findByLabelText('Select first.fits'));
    await userEvent.click(screen.getByRole('button', { name: 'Review 1 images' }));
    const review = await screen.findByRole('region', { name: 'Review contribution' });
    expect(review).toHaveTextContent('2.50 arcsec');
    expect(review).toHaveTextContent('Guiding RMSUnknown');
    images = [image('first'), image('later')];
    await act(() => client.invalidateQueries({ queryKey: ['db', 'rig-db'] }));
    expect(await screen.findByText('2 accepted images')).toBeVisible();
    expect(screen.getByLabelText('Select later.fits')).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Review 1 images' })).toBeEnabled();
    images = [image('later')];
    await act(() => client.invalidateQueries({ queryKey: ['db', 'rig-db'] }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Queue finalized contribution' })).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Review 0 images' })).toBeDisabled();
    expect(requests.some(r => r.operation === 'queue_report')).toBe(false);
  });
});
