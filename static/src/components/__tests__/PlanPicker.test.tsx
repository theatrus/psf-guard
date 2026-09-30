import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import PlanPicker from '../header/PlanPicker';
import { useCurrentPlan } from '../header/useCurrentPlan';
import type { DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const enabled = { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', acquisition_available: false, database_management: true };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const other = { id: '55555555-5555-4555-8555-555555555555', name: 'RC51', revision: 1 };
const target = (desired: number, accepted: number) => ({ name: 't', desired, acquired: accepted, accepted, rejected: 0, center: null, rotation_degrees: null });
const link = (slug: string, name: string, who: typeof rig, row: number, state: number, desired: number, accepted: number) => ({ catalog_slug: slug, catalog_name: name, rig: who, source_project_guid: 'g', source_row_id: row, source_name: 'M31', source_state: state, earliest_capture_s: null, latest_capture_s: null, targets: [target(desired, accepted)] });
const rows: DirectorPlanRow[] = [
  { project: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'M31', revision: 1 }, links: [link('c925', 'C925', rig, 7, 1, 40, 10), link('rc51', 'RC51', other, 3, 1, 40, 40)], progress: null, framing: null, plan: null, activation: null },
  { project: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'Pelican', revision: 1 }, links: [link('c925', 'C925', rig, 9, 3, 20, 20)], progress: null, framing: null, plan: null, activation: null },
  { project: { id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Bare', revision: 1 }, links: [], progress: null, framing: null, plan: null, activation: null },
];

function Probe() {
  const location = useLocation();
  const plan = useCurrentPlan();
  return <output data-testid="probe">{location.pathname}{location.search}|{plan.current?.project.name ?? 'none'}</output>;
}

function mount(route: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  server.use(
    http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
    http.get('/api/director/v1/plans', () => HttpResponse.json(ok({ rows, warnings: [] }))),
  );
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Probe /></MemoryRouter></QueryClientProvider>;
  return render(<PlanPicker />, { wrapper: Wrapper });
}

describe('Plan picker', () => {
  it('follows the review scope to its plan and opens the workspace for it', async () => {
    mount('/grid?db=rc51&project=3');
    const trigger = await screen.findByRole('button', { name: 'Plan: M31' });
    expect(trigger).toHaveTextContent('2 rigs');
    expect(screen.getByTestId('probe')).toHaveTextContent('|M31');
    fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog', { name: 'Choose a plan' });
    // Live plans first, closed ones folded, no database said plainly.
    expect(within(dialog).getByText('Bare')).toBeInTheDocument();
    expect(within(dialog).getByText('No database')).toBeInTheDocument();
    expect(within(dialog).getByText(/Closed plans/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: /^Bare/ }));
    expect(screen.getByTestId('probe')).toHaveTextContent('/director?db=rc51&project=3&directorProject=cccccccc-cccc-4ccc-8ccc-cccccccccccc|Bare');
  });

  it('ignores a plan named in the URL outside Planning, so review decides', async () => {
    mount('/grid?db=c925&project=9&directorProject=aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
    expect(await screen.findByRole('button', { name: 'Plan: Pelican' })).toBeInTheDocument();
  });

  it('names the plan the URL asks for and hops to a rig for review', async () => {
    mount('/director?directorProject=aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
    fireEvent.click(await screen.findByRole('button', { name: 'Plan: M31' }));
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search plans' }), { target: { value: 'm3' } });
    expect(screen.queryByText('Bare')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Review M31 on C925' }));
    expect(screen.getByTestId('probe')).toHaveTextContent('/grid?db=c925&project=7|M31');
  });

  it('offers a choice when nothing is in scope, and the way to every plan', async () => {
    mount('/?dbfilter=c925');
    const trigger = await screen.findByRole('button', { name: 'Plan: Choose a plan' });
    expect(screen.getByTestId('probe')).toHaveTextContent('|none');
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('button', { name: /All plans/ }));
    expect(screen.getByTestId('probe')).toHaveTextContent('/director?dbfilter=c925|none');
  });
});
