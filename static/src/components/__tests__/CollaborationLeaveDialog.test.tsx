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
  transfers: 1, nightly_intake: ['Community'], open_assignments: 6, coordinator_reachable: false,
  signed_in: ['RedCat'], not_signed_in: ['Askar'],
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
      http.get(leavePath, () => ok({ name: 'M31', collaboration, warnings: [], refusal: null })),
      http.post(leavePath, () => {
        assignmentsLeft = { imports: [] };
        return ok({
          state: 'left', name: 'M31', collaboration, assignments_ended: 6, warnings: [],
          coordinator: { host: 'collab.example', tasks: 2, declined: ['000000000004'], problems: ['Askar is not signed in to this collaboration.'] },
        });
      }),
    );
    mount(true, value => changes.push(value));
    fireEvent.click(await screen.findByRole('button', { name: 'Leave collaboration' }));
    const dialog = await screen.findByRole('dialog');
    expect(await screen.findByText(/comes from the collaboration at collab.example \(M31 halo, project 000000000002\)/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent('Ends 6 open assignments in the rig databases, so new frames no longer count toward the collaboration.');
    expect(dialog).toHaveTextContent('Removes 1 imported nightly assignment.');
    expect(dialog).toHaveTextContent("Deletes 1 report not yet sent. It will never reach the coordinator. To have it counted, deliver it first with Check in in the rig's Collaboration section.");
    expect(dialog).toHaveTextContent('Removes the record of 2 reports already sent. The coordinator keeps the reports it received.');
    expect(dialog).toHaveTextContent('5 frames in reports lose their protection. Removing rejects and deleting the project in the Library can then remove them.');
    expect(dialog).toHaveTextContent('Cancels 1 file transfer that has not finished.');
    expect(dialog).toHaveTextContent('Stops nightly intake on Community from importing the project.');
    expect(dialog).toHaveTextContent('PSF Guard declines the tasks of RedCat at collab.example. Askar is not signed in to collab.example, so leave or decline the project there for that rig too.');
    expect(dialog).toHaveTextContent('Its rig projects keep their targets, frames and grades');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Leave collaboration' }));
    const result = await within(dialog).findByText(/left the collaboration at collab.example/);
    expect(result).toHaveTextContent('M31 left the collaboration at collab.example (M31 halo, project 000000000002). It is now a plan of your own. Ended 6 open assignments.');
    expect(result).toHaveTextContent("collab.example declined 1 of the plan's 2 tasks. Askar is not signed in to this collaboration. Leave or decline the project on collab.example for the rest.");
    expect(changes).toEqual([[]]);
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    // The Leave button went with the assignments; focus moves to the line
    // that says what leaving did.
    const status = await screen.findByRole('status');
    expect(status).toHaveTextContent('M31 left the collaboration at collab.example');
    await waitFor(() => expect(status).toHaveFocus());
  });

  it('leaves even when a linked database cannot be reached, and says what stays open', async () => {
    const warning = 'The database for RedCat is not registered on this server, so the plan\'s assignments there stay open. PSF Guard no longer counts frames toward them. To end them, register the database again in Settings if it is missing, then activate the plan.';
    server.use(
      http.get(assignments, () => ok(imported)),
      http.get(leavePath, () => ok({ name: 'M31', collaboration, warnings: [warning], refusal: null })),
      http.post(leavePath, () => ok({
        state: 'left', name: 'M31', collaboration, assignments_ended: 0, warnings: [warning],
        coordinator: { host: 'collab.example', tasks: 1, declined: ['000000000004'], problems: [] },
      })),
    );
    mount(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Leave collaboration' }));
    const dialog = await screen.findByRole('dialog');
    expect(await within(dialog).findByText(/The database for RedCat is not registered/)).toHaveTextContent(warning);
    const leave = within(dialog).getByRole('button', { name: 'Leave collaboration' });
    expect(leave).toBeEnabled();
    fireEvent.click(leave);
    expect(await screen.findByText(/collab.example declined the plan's task\./)).toBeInTheDocument();
    expect(within(screen.getByRole('dialog')).getAllByRole('note').some(note => note.textContent === warning)).toBe(true);
  });

  it('explains why a plan of its own has nothing to leave', async () => {
    server.use(
      http.get(assignments, () => ok(imported)),
      http.get(leavePath, () => ok({ name: 'M31', collaboration: null, warnings: [], refusal: 'This plan did not come from a collaboration.' })),
    );
    mount(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Leave collaboration' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('This plan did not come from a collaboration.');
    expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'Leave collaboration' })).toBeDisabled();
  });
});
