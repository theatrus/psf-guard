import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';

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

const running = {
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
      http.get('/api/db/alpha/wbpp/runs/current', () => ok({ started: true, progress: running }))
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
});
