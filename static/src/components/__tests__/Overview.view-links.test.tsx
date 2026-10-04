import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

const now = Math.floor(Date.now() / 1000);

function project(id: number, name: string, targetCount: number) {
  return {
    id, guid: null, profile_id: 'profile', profile_name: 'Profile', name, display_name: name, has_files: true, state: 1,
    target_count: targetCount, total_images: 10, accepted_images: 5, rejected_images: 2, pending_images: 3, total_desired: 20,
    files_found: 10, files_missing: 0, date_range: { earliest: now - 86_400, latest: now - 3_600 }, filters_used: ['Ha'], recent_images: [],
  };
}

function target(id: number, projectId: number, name: string) {
  return {
    id, name, active: true, project_id: projectId, project_name: '', image_count: 5, accepted_count: 3, rejected_count: 1,
    pending_count: 1, total_desired: 10, files_found: 5, files_missing: 0, has_files: true,
    date_range: { earliest: now - 86_400, latest: now - 3_600 }, filters_used: ['Ha'],
  };
}

function Location() {
  const location = useLocation();
  return <output aria-label="Location">{`${location.pathname}${location.search}`}</output>;
}

function renderLibrary(density: 'compact' | 'detailed') {
  setDisplayPreferences({ showNightChip: true, showAllChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: density });
  server.use(
    http.get('/api/databases', () => ok([{ id: 'askar', name: 'Askar', path: '/a.sqlite' }])),
    http.get('/api/db/:dbId/projects/overview', () => ok([project(1, 'Heart Nebula', 1), project(2, 'Cygnus Wall', 2)])),
    http.get('/api/db/:dbId/targets/overview', () =>
      ok([target(7, 1, 'IC 1805'), target(8, 2, 'NGC 7000 West'), target(9, 2, 'NGC 7000 East')])
    ),
    http.get('/api/db/:dbId/stats/overall', () => ok({ total_projects: 2, active_projects: 2, total_targets: 3, active_targets: 3, total_images: 20, accepted_images: 10, rejected_images: 4, pending_images: 6, total_desired: 40, files_found: 20, files_missing: 0, unique_filters: ['Ha'], date_range: { earliest: now - 86_400, latest: now - 3_600 }, recent_activity: [] })),
    http.get('/api/settings/export', () => ok({ default_layout: 'flat' })),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="*" element={<Location />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('Library view links', () => {
  it('opens Sequence and Stacks from a compact row, at the only target when there is one', async () => {
    renderLibrary('compact');
    const rows = await screen.findAllByTestId('library-row');
    const heart = rows.find((row) => within(row).queryByText('Heart Nebula'))!;
    fireEvent.click(within(heart).getByRole('button', { name: 'Open Heart Nebula sequence' }));
    expect(await screen.findByRole('status', { name: 'Location' })).toHaveTextContent(
      '/sequence?db=askar&project=1&target=7'
    );
  });

  it('opens a project with several targets at the project, and a target card at its target', async () => {
    renderLibrary('detailed');
    const wall = (await screen.findByText('Cygnus Wall', { selector: '.project-title' })).closest('.project-card') as HTMLElement;
    expect(within(wall).getByRole('button', { name: 'Open Cygnus Wall image grid' })).toBeInTheDocument();
    fireEvent.click(within(wall).getByRole('button', { name: 'Open Cygnus Wall stacks' }));
    expect(await screen.findByRole('status', { name: 'Location' })).toHaveTextContent('/stacks?db=askar&project=2');
  });

  it('opens one target of a project in Sequence', async () => {
    renderLibrary('detailed');
    fireEvent.click(await screen.findByRole('button', { name: 'Open NGC 7000 East sequence' }));
    expect(await screen.findByRole('status', { name: 'Location' })).toHaveTextContent(
      '/sequence?db=askar&project=2&target=9'
    );
    setDisplayPreferences({ showNightChip: true, showAllChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' });
  });
});
