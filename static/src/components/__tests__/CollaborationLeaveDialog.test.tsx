import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import CollaborationActivation from '../director/CollaborationActivation';
import type { DirectorPlanCollaboration } from '../../api/directorTypes';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const PLAN = '11111111-1111-4111-8111-111111111111';
const leavePath = `/api/director/v1/projects/${PLAN}/collaboration/leave`;
const assignments = `/api/director/v1/projects/${PLAN}/activation/collaboration`;

const collaboration: DirectorPlanCollaboration = {
  host: 'collab.example', base_url: 'https://collab.example/', remote_project_id: '000000000002',
  remote_project_name: 'M31 halo', imports: 1, unsent_reports: 1, sent_reports: 2, reported_frames: 5,
  transfers: 1, nightly_intake: ['Community'], open_assignments: 6, coordinator_reachable: true,
};
const imported = {
  imports: [{ import_id: 'i-1', rig_id: 'r-1', rig_name: 'RedCat', source_digest: 'd', night: '2026-10-05', task_id: '000000000004', version: 2, demands: [] }],
};

function mount(canLeave: boolean, onChange: (value: unknown) => void = () => {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  render(<CollaborationActivation projectId={PLAN} value={[]} onChange={onChange} disabled={false} canLeave={canLeave} />, { wrapper });
}

describe('Leaving a collaboration', () => {
  it('is offered only where rig databases can change', async () => {
    server.use(http.get(assignments, () => ok(imported)));
    mount(false);
    await screen.findByText(/RedCat · 2026-10-05/);
    expect(screen.queryByRole('button', { name: 'Leave collaboration' })).not.toBeInTheDocument();
  });

  it('shows what leaving removes and keeps, then what the coordinator was told', async () => {
    let assignmentsLeft = imported;
    const changes: unknown[] = [];
    server.use(
      http.get(assignments, () => ok(assignmentsLeft)),
      http.get(leavePath, () => ok({ name: 'M31', collaboration, refusal: null })),
      http.post(leavePath, () => {
        assignmentsLeft = { imports: [] };
        return ok({
          state: 'left', name: 'M31', collaboration, assignments_ended: 6,
          coordinator: { host: 'collab.example', declined: [], problems: ['The saved sign-in for Community is missing.'] },
        });
      }),
    );
    mount(true, value => changes.push(value));
    fireEvent.click(await screen.findByRole('button', { name: 'Leave collaboration' }));
    const dialog = await screen.findByRole('dialog');
    expect(await screen.findByText(/comes from the collaboration at collab.example \(M31 halo, project 000000000002\)/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent('Ends 6 open assignments in the rig databases, so new frames no longer count toward the collaboration.');
    expect(dialog).toHaveTextContent('Removes 1 imported nightly assignment and 1 report not yet sent and the record of 2 reports already sent. The coordinator keeps the reports it received.');
    expect(dialog).toHaveTextContent('Cancels 1 file transfer that has not finished.');
    expect(dialog).toHaveTextContent('Stops nightly intake on Community from importing the project.');
    expect(dialog).toHaveTextContent("PSF Guard declines the plan's tasks at collab.example");
    expect(dialog).toHaveTextContent('Its rig projects keep their targets, frames and grades');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Leave collaboration' }));
    const result = await screen.findByText(/left the collaboration at collab.example/);
    expect(result).toHaveTextContent('M31 left the collaboration at collab.example (M31 halo, project 000000000002) and is now a plan of your own. Ended 6 open assignments.');
    expect(result).toHaveTextContent('collab.example was not told: The saved sign-in for Community is missing. Leave or decline the project on collab.example too.');
    expect(changes).toEqual([[]]);
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('explains a refusal and keeps Leave collaboration disabled', async () => {
    server.use(
      http.get(assignments, () => ok(imported)),
      http.get(leavePath, () => ok({ name: 'M31', collaboration, refusal: 'The database for RedCat is not registered on this server. Register it again in Settings, then leave the collaboration.' })),
    );
    mount(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Leave collaboration' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The database for RedCat is not registered on this server.');
    expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'Leave collaboration' })).toBeDisabled();
  });
});
