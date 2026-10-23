import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { CLOSED_PROJECT_STATE, INACTIVE_PROJECT_STATE } from '../../utils/projectNavigation';

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
  // These tests read the full cards; the Library opens compact by default.
  beforeEach(() => { setDisplayPreferences({ showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'detailed' }); });
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
    // A screen reader hears which project the user came from.
    expect(screen.getByRole('button', { name: 'Open NGC 6820 image grid' })).toHaveAttribute(
      'aria-current',
      'true'
    );
    expect(
      screen.getByRole('button', { name: 'Open Sh2 86 image grid' })
    ).not.toHaveAttribute('aria-current');
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
    expect(archived.closest('.project-archive-item')).toHaveClass('is-current');
    // A screen reader hears it on the row's open button, which takes focus.
    expect(archived.closest('button')).toHaveAttribute('aria-current', 'true');
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(1));
  });

  it('reopens an archived project as Inactive and says so beside the archive', async () => {
    let state = CLOSED_PROJECT_STATE;
    const writes: unknown[] = [];
    useOverviewData([project(1, 'Sh2 86')]);
    server.use(
      http.get('/api/db/:dbId/projects/overview', () => ok([project(1, 'Sh2 86'), project(9, 'Old survey', state)])),
      http.put('/api/db/:dbId/projects/:projectId', async ({ request, params }) => {
        expect(params).toMatchObject({ dbId: 'test', projectId: '9' });
        const body = await request.json() as { state: number };
        writes.push(body);
        state = body.state;
        return ok({ updated: true });
      }),
    );
    Element.prototype.scrollIntoView = vi.fn();

    render(<Overview />, { wrapper: wrapper('/?db=test&project=9') });

    fireEvent.click(await screen.findByRole('button', { name: 'Reopen Old survey on Demo catalog' }));
    // Deleting needs a server that allows database changes; this one does not.
    expect(screen.queryByRole('button', { name: /^Delete Old survey/ })).not.toBeInTheDocument();
    expect(await screen.findByRole('status')).toHaveTextContent('Old survey is now Inactive on Demo catalog.');
    expect(screen.getByRole('status')).toHaveFocus();
    expect(writes).toEqual([{ state: INACTIVE_PROJECT_STATE }]);
    // Back in the list, the project leaves the archive.
    await waitFor(() => expect(document.querySelector('.project-archive')).toBeNull());
    expect(screen.getByText('Old survey').closest('.project-card')).not.toBeNull();
    // The notice outlasts the archive it came from, and keeps focus.
    expect(screen.getByRole('status')).toHaveTextContent('Old survey is now Inactive on Demo catalog.');
    expect(screen.getByRole('status')).toHaveFocus();
  });

  it('deletes an archived project after its preview, and says what went', async () => {
    let deleted = false;
    useOverviewData([project(1, 'Sh2 86')]);
    server.use(
      http.get('/api/info', () => ok({ version: 'test', cache_directory: '/tmp', allow_database_management: true })),
      http.get('/api/db/:dbId/projects/overview', () => ok(deleted
        ? [project(1, 'Sh2 86')]
        : [project(1, 'Sh2 86'), project(9, 'Old survey', CLOSED_PROJECT_STATE)])),
      http.get('/api/db/:dbId/deleted-projects', () => ok(deleted
        ? [{ id: 1, project_id: 9, name: 'Old survey', removed_at: 1_705_400_000, frames: 0 }]
        : [])),
      http.get('/api/db/:dbId/projects/9/removal', () => ok({
        project_id: 9, name: 'Old survey', guid: null, state: 3, targets: 1, exposure_plans: 2,
        frames: { total: 0, accepted: 0, rejected: 0, pending: 0 }, protected_frames: 0,
        digest: 'd', linked_plan: null, synced_from: null, allowed: true,
      })),
      http.post('/api/db/:dbId/projects/9/removal', () => {
        deleted = true;
        return ok({ state: 'removed', name: 'Old survey', targets: 1, exposure_plans: 2, frames: 0, plan: null, plan_error: null });
      }),
    );
    Element.prototype.scrollIntoView = vi.fn();

    render(<Overview />, { wrapper: wrapper('/?db=test&project=9') });

    fireEvent.click(await screen.findByRole('button', { name: 'Delete Old survey from Demo catalog' }));
    expect(await screen.findByText('1 target and 2 exposure plans')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete project' }));
    const notice = await screen.findByText('Deleted Old survey from Demo catalog, with 1 target and 0 frames.');
    await waitFor(() => expect(notice).toHaveFocus());
    await waitFor(() => expect(document.querySelector('.project-archive-item [aria-current]')).toBeNull());
    // It is listed among the deleted projects, which syncs and imports leave out.
    expect(await screen.findByRole('button', { name: /Deleted projects/ })).toHaveAttribute('aria-expanded', 'false');
  });

  it('moves focus to the notice after a deletion from the Edit project panel', async () => {
    let deleted = false;
    useOverviewData([project(1, 'Sh2 86'), project(2, 'NGC 6820')]);
    server.use(
      http.get('/api/info', () => ok({ version: 'test', cache_directory: '/tmp', allow_database_management: true })),
      http.get('/api/db/:dbId/projects/overview', () => ok(deleted ? [project(1, 'Sh2 86')] : [project(1, 'Sh2 86'), project(2, 'NGC 6820')])),
      http.get('/api/db/:dbId/deleted-projects', () => ok([])),
      http.get('/api/db/:dbId/projects/2/removal', () => ok({
        project_id: 2, name: 'NGC 6820', guid: null, state: 1, targets: 1, exposure_plans: 1,
        frames: { total: 10, accepted: 5, rejected: 2, pending: 3 }, protected_frames: 0,
        digest: 'd', linked_plan: null, synced_from: null, allowed: true,
      })),
      http.post('/api/db/:dbId/projects/2/removal', () => {
        deleted = true;
        return ok({ state: 'removed', name: 'NGC 6820', targets: 1, exposure_plans: 1, frames: 10, plan: null, plan_error: null });
      }),
    );
    Element.prototype.scrollIntoView = vi.fn();

    render(<Overview />, { wrapper: wrapper('/?db=test&project=2') });

    const card = (await screen.findByText('NGC 6820')).closest('.project-card') as HTMLElement;
    fireEvent.click(within(card).getByRole('button', { name: /Edit project/ }));
    fireEvent.click(within(card).getByRole('button', { name: 'Delete project...' }));
    fireEvent.change(await screen.findByRole('textbox', { name: /Type NGC 6820/ }), { target: { value: 'NGC 6820' } });
    fireEvent.click(screen.getByRole('button', { name: 'Delete project' }));
    const notice = await screen.findByText('Deleted NGC 6820 from Demo catalog, with 1 target and 10 frames. The image files are still on disk.');
    await waitFor(() => expect(notice).toHaveFocus());
  });
});
