import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import DirectorDashboard from '../director/DirectorDashboard';
import { describeNow, formatAge } from '../director/dashboardModel';
import type { DirectorRigStatusView } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const NOW = 1_800_000_000_000;
const rig = (id: string, name: string): DirectorRigStatusView['rig'] => ({ id, name, revision: 1 });
const project = { id: 'pppppppp-pppp-4ppp-8ppp-pppppppppppp', name: 'Heart Nebula', revision: 1 };
const online: DirectorRigStatusView = {
  rig: rig('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'RedCat'), catalog_slug: 'redcat', catalog_name: 'RedCat 61',
  status: { rig_id: 'a', session_id: 's1', reported_at_ms: NOW - 40_000, received_at_ms: NOW - 40_000,
    payload: { phase: 'exposing', target_name: 'IC 1805 r1c1', operation: 'exposure', operation_started_ms: NOW - 200_000, queue_depth: 2, errors: ['guider lost the star once'] } },
  status_age_ms: 40_000, status_stale: false, checkins: [],
  contacts: { program_pull: { at_ms: NOW - 3_600_000, detail: 'abcdef0123456789' }, check_in: { at_ms: NOW - 120_000, detail: 'ledger-1' }, status: { at_ms: NOW - 40_000, detail: 's1' } },
  connectivity: { state: 'online', last_contact_ms: NOW - 40_000, age_ms: 40_000 }, assignments: [{ project, activation_revision: 2, applied_at_ms: NOW - 86_400_000 }], pending_receipts: 5,
};
const quiet: DirectorRigStatusView = {
  rig: rig('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', 'C925'), catalog_slug: 'c925', catalog_name: 'C925',
  status: { rig_id: 'b', session_id: 's9', reported_at_ms: NOW - 3_000_000, received_at_ms: NOW - 3_000_000, payload: { phase: 'exposing' } },
  status_age_ms: 3_000_000, status_stale: true, checkins: [],
  contacts: { program_pull: null, check_in: null, status: { at_ms: NOW - 3_000_000, detail: 's9' } },
  connectivity: { state: 'offline', last_contact_ms: NOW - 3_000_000, age_ms: 3_000_000 }, assignments: [], pending_receipts: 0,
};
const never: DirectorRigStatusView = {
  rig: rig('cccccccc-cccc-4ccc-8ccc-cccccccccccc', 'Desert'), catalog_slug: 'desert', catalog_name: 'Desert copy', status: null, status_age_ms: null, status_stale: false, checkins: [],
  contacts: { program_pull: null, check_in: null, status: null }, connectivity: { state: 'never', last_contact_ms: null, age_ms: null }, assignments: [], pending_receipts: 0,
};

function mount(rows: DirectorRigStatusView[]) {
  server.use(http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok(rows))));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}><MemoryRouter initialEntries={['/director?db=redcat']}>{children}</MemoryRouter></QueryClientProvider>;
  }
  return render(<DirectorDashboard />, { wrapper: Wrapper });
}

describe('Director live dashboard', () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ['Date'] }); vi.setSystemTime(NOW); });
  afterEach(() => vi.useRealTimers());

  it('shows each rig with its connectivity, what it is doing, contact ages and assignments', async () => {
    mount([online, quiet, never]);
    expect(await screen.findByTestId('director-dashboard')).toBeInTheDocument();
    expect(screen.getByText('Online')).toBeInTheDocument();
    expect(screen.getByText('exposing, IC 1805 r1c1, exposure for 3 min, 2 queued')).toBeInTheDocument();
    expect(screen.getByText('guider lost the star once')).toBeInTheDocument();
    expect(screen.getByText('5 saved, ungraded')).toBeInTheDocument();
    expect(screen.getByText('rev abcdef01')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Heart Nebula' })).toHaveAttribute('href', `/director?db=redcat&directorProject=${project.id}`);
    // An old report is history, not the present.
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(screen.getByText('exposing (stale, reported 50 min ago)')).toBeInTheDocument();
    expect(screen.getByText('Never seen')).toBeInTheDocument();
    expect(screen.getByText('No report yet')).toBeInTheDocument();
    expect(screen.getAllByText('never')).toHaveLength(5);
    expect(screen.getAllByText('nothing activated')).toHaveLength(2);
  });

  it('says when no rig exists yet', async () => {
    mount([]);
    expect(await screen.findByText(/No rig yet/)).toBeInTheDocument();
  });

  it('formats ages and reports plainly', () => {
    expect(formatAge(20_000)).toBe('20 s ago');
    expect(formatAge(15 * 60_000)).toBe('15 min ago');
    expect(formatAge(5 * 3_600_000)).toBe('5 h ago');
    expect(formatAge(3 * 86_400_000)).toBe('3 d ago');
    expect(describeNow({ ...never, connectivity: { state: 'online', last_contact_ms: NOW, age_ms: 0 } }, NOW)).toBe('No status report yet');
    expect(describeNow({ ...online, status: { ...online.status!, payload: { state: 'idle', wait_reason: 'clouds', safety: 'unsafe' } } }, NOW)).toBe('idle, waiting: clouds, safety unsafe');
  });
});
