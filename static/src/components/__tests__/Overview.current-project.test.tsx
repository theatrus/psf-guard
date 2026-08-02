import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { CLOSED_PROJECT_STATE } from '../../utils/projectNavigation';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function project(id: number, name: string, state = 1) {
  return {
    id,
    profile_id: 'profile',
    profile_name: 'Profile',
    name,
    display_name: name,
    has_files: true,
    state,
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

function wrapper(route: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

function useOverviewData(projects: ReturnType<typeof project>[]) {
  server.use(
    http.get('/api/databases', () => ok([
      { id: 'test', name: 'Demo catalog', path: '/demo.sqlite' },
    ])),
    http.get('/api/db/:dbId/projects/overview', () => ok(projects)),
    http.get('/api/db/:dbId/targets/overview', () => ok([])),
    http.get('/api/db/:dbId/stats/overall', () => ok({
      total_projects: projects.length,
      active_projects: projects.length,
      total_targets: 1,
      active_targets: 1,
      total_images: 10,
      accepted_images: 5,
      rejected_images: 2,
      pending_images: 3,
      total_desired: 20,
      files_found: 10,
      files_missing: 0,
      unique_filters: ['Ha'],
      date_range: { earliest: 1_705_000_000, latest: 1_705_352_400 },
      recent_activity: [],
    })),
  );
}

describe('Overview return scope', () => {
  it('marks and reveals the project the user came from', async () => {
    useOverviewData([project(1, 'Sh2 86'), project(2, 'NGC 6820')]);
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;

    render(<Overview />, { wrapper: wrapper('/?db=test&project=2') });

    const current = await screen.findByText('NGC 6820');
    const card = current.closest('.project-card');
    expect(card).toHaveAttribute('data-current-project', 'true');
    expect(
      screen.getByText('Sh2 86').closest('.project-card')
    ).not.toHaveAttribute('data-current-project');
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(1));
  });

  it('leaves every card unmarked without a project in the URL', async () => {
    useOverviewData([project(1, 'Sh2 86')]);
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;

    const { container } = render(<Overview />, { wrapper: wrapper('/') });

    await screen.findByText('Sh2 86');
    expect(container.querySelector('[data-current-project]')).toBeNull();
    expect(scrollIntoView).not.toHaveBeenCalled();
  });

  it('opens the archive when the project the user came from is archived', async () => {
    useOverviewData([project(1, 'Sh2 86'), project(9, 'Old survey', CLOSED_PROJECT_STATE)]);
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;

    render(<Overview />, { wrapper: wrapper('/?db=test&project=9') });

    const archived = await screen.findByText('Old survey');
    expect(archived.closest('.project-archive-item')).toHaveAttribute(
      'data-current-project',
      'true'
    );
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(1));
  });
});
