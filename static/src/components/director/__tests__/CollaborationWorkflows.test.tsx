import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
}
describe('Collaboration work transfer', () => {
  it('requires explicit observing-night context and a preview before applying', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>; requests.push(body);
      const data = body.operation === 'tonight' ? { shares: [share] } : body.operation === 'preview'
        ? { preview: { review_digest: 'review-current', action: 'create', acquisition_enabled: false }, plan: { project_id: 'project', night: '2026-10-05', share } }
        : {};
      return HttpResponse.json({ success: true, data });
    }));
    setup();
    expect(screen.getByRole('button', { name: 'Pull nightly work' })).toBeDisabled();
    await userEvent.type(screen.getByLabelText('Night'), '2026-10-05');
    await userEvent.type(screen.getByLabelText('Moon illumination (%)'), '12');
    await userEvent.type(screen.getByLabelText('Moon above horizon (%)'), '30');
    await userEvent.click(screen.getByRole('button', { name: 'Pull nightly work' }));
    expect(screen.queryByRole('button', { name: 'Import draft' })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole('button', { name: 'Review import' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Import draft' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'apply', task: share.task_id, night: { night: '2026-10-05', moon: 0.12, moon_up: 0.3 }, review_digest: 'review-current' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Imported as an inactive project draft');
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
    await userEvent.click(screen.getByRole('button', { name: 'Contribution reports' }));
    await screen.findByRole('option', { name: 'M31 (2026-10-05)' });
    await userEvent.selectOptions(screen.getByLabelText('Imported visit'), 'import');
    await userEvent.selectOptions(screen.getByLabelText('Rig database'), 'rig-db');
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
    await userEvent.click(screen.getByRole('button', { name: 'Contribution reports' }));
    await screen.findByRole('option', { name: 'M31 (2026-10-05)' });
    await userEvent.selectOptions(screen.getByLabelText('Imported visit'), 'import');
    await userEvent.selectOptions(screen.getByLabelText('Rig database'), 'rig-db');
    await screen.findByRole('option', { name: '7' });
    await userEvent.selectOptions(screen.getByLabelText('Remote panel'), '7');
    await userEvent.click(await screen.findByLabelText('Select old-panel.fits'));
    await userEvent.click(screen.getByRole('button', { name: 'Review 1 images' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Queue finalized contribution' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'queue_report', selection: { import_id: 'import', catalog: 'rig-db', panel: 7, image_guids: ['old-image'], observing_night: '2026-10-05', source_digest: 'original-revision' }, review_digest: 'old-evidence' }));
  });
});
