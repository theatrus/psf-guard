import { useState } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import type { DirectorScheduleGoal, NightOverride } from '../../../api/nightPlanTypes';
import OverrideEditor from '../OverrideEditor';
import { clockOnNight } from '../nightsModel';
import { night, rig } from './fixtures';

const ok = (data: unknown) => ({ success: true, data, error: null });
const goals: DirectorScheduleGoal[] = [{ goal_id: 'e1', plan_id: 'p1', plan_name: 'Heart Nebula', target_guid: 't1', exposureplan_guid: 'e1', project_id: 7, target_id: 70, exposure_plan_id: 700, target_name: 'IC 1805', filter: 'Ha' }];

/** The editor with the open edit held as the night panel holds it. */
function Harness({ zone = null }: { zone?: string | null }) {
  const [edit, setEdit] = useState<{ base: number; entries: NightOverride[] } | null>(null);
  const tonight = night(0);
  return <OverrideEditor rig={rig({ runtime: 'director', time_zone: zone })} night={tonight} scheduled={undefined} goals={goals}
    tonight={false} now={tonight.noon_ms} canEdit draft={edit?.entries ?? null} base={edit?.base ?? null}
    onStart={(entries, base) => setEdit({ base, entries })}
    onDraft={entries => setEdit(current => entries === null || !current ? null : { ...current, entries })} />;
}

describe('OverrideEditor', () => {
  it('saves against the revision the edit started from, and drops the edit when that is gone', async () => {
    let revision = 3;
    let reads = 0;
    const saves: Array<{ expected_revision: number }> = [];
    server.use(
      http.get('/api/director/v1/rigs/:rig/night-overrides/:date', ({ params }) => {
        reads += 1;
        return HttpResponse.json(ok({ date: params.date, revision, entries: [], runtime: 'director' }));
      }),
      http.put('/api/director/v1/rigs/:rig/night-overrides/:date', async ({ request }) => {
        saves.push(await request.json() as { expected_revision: number });
        return HttpResponse.json({ success: false, data: null, error: 'Director record conflicts with stored content; reload before retrying' }, { status: 409 });
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
    const editor = screen.getByRole('region', { name: 'Overrides' });
    await waitFor(() => expect(within(editor).getByRole('button', { name: 'Edit night' })).toBeEnabled());
    fireEvent.click(within(editor).getByRole('button', { name: 'Edit night' }));
    // Someone else saves while this edit is open, and the night is read again.
    revision = 7;
    await client.invalidateQueries({ queryKey: ['nightOverrides'] });
    await waitFor(() => expect(reads).toBe(2));
    fireEvent.click(within(editor).getByLabelText('Hold', { exact: true }));
    fireEvent.click(within(editor).getByRole('button', { name: 'Add' }));
    expect(within(editor).getByRole('listitem')).toHaveTextContent('Hold the rig all night');
    fireEvent.click(within(editor).getByRole('button', { name: 'Save overrides' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].expected_revision).toBe(3);
    expect(await within(editor).findByRole('alert')).toHaveTextContent("The night's overrides changed since the edit started");
    expect(within(editor).getByRole('button', { name: 'Edit night' })).toBeInTheDocument();
    expect(within(editor).queryByRole('button', { name: 'Save overrides' })).toBeNull();
  });

  it('refuses a hold that falls outside the night', async () => {
    server.use(http.get('/api/director/v1/rigs/:rig/night-overrides/:date', ({ params }) =>
      HttpResponse.json(ok({ date: params.date, revision: 0, entries: [], runtime: 'director' }))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    // A site fourteen hours east of UTC: some of its clock times fall
    // outside a night that runs from noon to noon in the test's clock.
    const zone = 'Pacific/Kiritimati';
    render(<QueryClientProvider client={client}><Harness zone={zone} /></QueryClientProvider>);
    const editor = screen.getByRole('region', { name: 'Overrides' });
    await waitFor(() => expect(within(editor).getByRole('button', { name: 'Edit night' })).toBeEnabled());
    fireEvent.click(within(editor).getByRole('button', { name: 'Edit night' }));
    fireEvent.click(within(editor).getByLabelText('Hold', { exact: true }));
    const tonight = night(0);
    const clocks = Array.from({ length: 48 }, (_, half) => `${String(Math.floor(half / 2)).padStart(2, '0')}:${half % 2 ? '30' : '00'}`);
    const value = clocks.find(clock => {
      const at = clockOnNight(tonight.date, clock, zone)!;
      return at < tonight.noon_ms || at >= tonight.noon_ms + 86_400_000;
    })!;
    expect(value).toBeDefined();
    fireEvent.change(within(editor).getByLabelText('From'), { target: { value } });
    fireEvent.click(within(editor).getByRole('button', { name: 'Add' }));
    expect(within(editor).getByRole('alert')).toHaveTextContent('falls outside this night');
    expect(within(editor).queryByRole('listitem')).toBeNull();
  });
});
