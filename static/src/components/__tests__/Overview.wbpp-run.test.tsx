import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import type { WbppRunProgress } from '../../api/types';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function project(id: number, name: string) {
  return {
    id,
    profile_id: 'profile',
    profile_name: 'Profile',
    name,
    display_name: name,
    has_files: true,
    state: 1,
    target_count: 1,
    total_images: 10,
    accepted_images: 5,
    rejected_images: 2,
    pending_images: 3,
    total_desired: 20,
    files_found: 10,
    files_missing: 0,
    date_range: { earliest: 1_705_000_000, latest: 1_705_352_400 },
    filters_used: ['Ha'],
    recent_images: [],
  };
}

const stats = {
  total_projects: 2,
  active_projects: 2,
  total_targets: 2,
  active_targets: 2,
  total_images: 20,
  accepted_images: 10,
  rejected_images: 4,
  pending_images: 6,
  total_desired: 40,
  files_found: 20,
  files_missing: 0,
  unique_filters: ['Ha'],
  date_range: { earliest: 1_705_000_000, latest: 1_705_352_400 },
  recent_activity: [],
};

const running: WbppRunProgress = {
  running: true,
  stage: 'running',
  scope: 'Sh2 86',
  work_dir: '/runs/alpha/Sh2_86-1',
  output_dir: '/runs/alpha/Sh2_86-1/wbpp-out',
  free_bytes_at_start: null,
  options: null,
  frames: 40,
  lights: 30,
  missing_files: 0,
  command: null,
  pid: 4242,
  started_at: 1_705_352_400,
  finished_at: null,
  exit_code: null,
  wbpp_stage: 'Begin registration of light frames',
  wbpp_steps: 3,
  wbpp_elapsed: null,
  log_path: null,
  log_tail: [],
  log_errors: [],
  outputs: [],
  error: null,
  project_id: 1,
  publish: null,
};

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/']}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

describe('Overview WBPP run visibility', () => {
  it('shows a database’s run under way in a fresh tab, on its line and on the project’s action', async () => {
    server.use(
      http.get('/api/databases', () =>
        ok([{ id: 'alpha', name: 'Alpha catalog', path: '/alpha.sqlite' }])
      ),
      http.get('/api/info', () =>
        ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })
      ),
      http.get('/api/db/:dbId/projects/overview', () =>
        ok([project(1, 'Sh2 86'), project(2, 'NGC 6820')])
      ),
      http.get('/api/db/:dbId/targets/overview', () => ok([])),
      http.get('/api/db/:dbId/stats/overall', () => ok(stats)),
      http.get('/api/db/alpha/wbpp/runs/current', () =>
        ok({ started: true, queued: [], progress: running })
      )
    );
    render(<Overview />, { wrapper: wrapper() });

    // The line above the projects, with WBPP's own step.
    const line = await screen.findByRole('button', {
      name: 'WBPP Sh2 86: Begin registration of light frames',
    });
    expect(line).toBeInTheDocument();
    // The stacked project's action says so; the other project's does not.
    expect(await screen.findByText('⚗ Stacking in WBPP…')).toBeInTheDocument();
    expect(screen.getAllByText('⚗ Stack in WBPP')).toHaveLength(1);
  });

  it('lists a run in line with its place, lets it leave, and clears a finished run', async () => {
    let queued = [
      { id: 'q1', scope: 'NGC 6820', project_id: 2, target_id: null, position: 1, queued_at: 1 },
    ];
    let progress: WbppRunProgress = { ...running, running: false, stage: 'complete', finished_at: 1_705_356_000, wbpp_elapsed: '1:02:03' };
    let dismissed = 0;
    server.use(
      http.get('/api/databases', () =>
        ok([{ id: 'alpha', name: 'Alpha catalog', path: '/alpha.sqlite' }])
      ),
      http.get('/api/info', () =>
        ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })
      ),
      http.get('/api/db/:dbId/projects/overview', () =>
        ok([project(1, 'Sh2 86'), project(2, 'NGC 6820')])
      ),
      http.get('/api/db/:dbId/targets/overview', () => ok([])),
      http.get('/api/db/:dbId/stats/overall', () => ok(stats)),
      http.get('/api/db/alpha/wbpp/runs/current', () => ok({ started: false, queued, progress })),
      http.delete('/api/db/alpha/wbpp/runs/queue/q1', () => {
        queued = [];
        return ok({ started: false, queued, progress });
      }),
      http.post('/api/db/alpha/wbpp/runs/current/dismiss', () => {
        dismissed += 1;
        progress = { ...progress, stage: '', finished_at: null, scope: '' };
        return ok({ started: false, queued, progress });
      })
    );
    render(<Overview />, { wrapper: wrapper() });

    expect(
      await screen.findByRole('button', { name: 'WBPP Sh2 86 finished: 0 masters in 1:02:03' })
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'WBPP NGC 6820 is next in line' })).toBeInTheDocument();
    expect(screen.getByText('⚗ Queued for WBPP')).toBeInTheDocument();
    expect(screen.getByText('⚗ WBPP masters ready')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Remove NGC 6820 from the WBPP queue' }));
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'WBPP NGC 6820 is next in line' })).not.toBeInTheDocument()
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Dismiss WBPP Sh2 86 finished: 0 masters in 1:02:03' })
    );
    await waitFor(() => expect(dismissed).toBe(1));
    await waitFor(() =>
      expect(screen.queryByText('⚗ WBPP masters ready')).not.toBeInTheDocument()
    );
    expect(screen.getAllByText('⚗ Stack in WBPP')).toHaveLength(2);
  });
});
