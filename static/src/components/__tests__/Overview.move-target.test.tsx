import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

const now = Math.floor(Date.now() / 1000);

function project(id: number, name: string) {
  return {
    id, guid: null, profile_id: 'profile', profile_name: 'Profile', name, display_name: name, has_files: true, state: 1,
    target_count: 1, total_images: 10, accepted_images: 5, rejected_images: 2, pending_images: 3, total_desired: 20,
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

function renderLibrary() {
  setDisplayPreferences({ showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'detailed' });
  const targetUpdates: unknown[] = [];
  server.use(
    http.get('/api/databases', () => ok([{ id: 'askar', name: 'Askar', path: '/a.sqlite' }])),
    http.get('/api/db/:dbId/projects/overview', () => ok([project(1, 'Heart Nebula'), project(2, 'Cygnus Wall')])),
    http.get('/api/db/:dbId/targets/overview', () => ok([target(7, 1, 'IC 1805'), target(8, 2, 'NGC 7000')])),
    http.get('/api/db/:dbId/stats/overall', () => ok({ total_projects: 2, active_projects: 2, total_targets: 2, active_targets: 2, total_images: 20, accepted_images: 10, rejected_images: 4, pending_images: 6, total_desired: 40, files_found: 20, files_missing: 0, unique_filters: ['Ha'], date_range: { earliest: now - 86_400, latest: now - 3_600 }, recent_activity: [] })),
    http.get('/api/settings/export', () => ok({ default_layout: 'flat' })),
    http.get('/api/db/:dbId/organization/destinations', () => ok({
      projects: [
        { id: 1, name: 'Heart Nebula', profile_id: 'profile' },
        { id: 2, name: 'Cygnus Wall', profile_id: 'profile' },
      ],
      targets: [{ id: 7, project_id: 1, name: 'IC 1805' }, { id: 8, project_id: 2, name: 'NGC 7000' }],
    })),
    http.put('/api/db/:dbId/targets/:targetId', async ({ request }) => {
      targetUpdates.push(await request.json());
      return ok({ updated: true, images_moved: 0 });
    }),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={['/']}>
        <Overview />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return { targetUpdates };
}

describe('Library target editing', () => {
  it('opens the move preview from Edit target instead of moving at once', async () => {
    const { targetUpdates } = renderLibrary();
    fireEvent.click(await screen.findByRole('button', { name: 'Edit target IC 1805' }));
    expect(screen.queryByRole('combobox', { name: /Move/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Move to another project...' }));
    const dialog = await screen.findByRole('dialog', { name: 'Move target' });
    expect(within(dialog).getByText('IC 1805')).toBeInTheDocument();
    await within(dialog).findByRole('option', { name: 'Cygnus Wall' });
    expect(within(dialog).queryByRole('option', { name: 'Heart Nebula' })).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Target name' })).not.toBeInTheDocument();
    expect(targetUpdates).toEqual([]);
    setDisplayPreferences({ showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' });
  });

  it('saves a typed name before opening the move preview', async () => {
    const { targetUpdates } = renderLibrary();
    fireEvent.click(await screen.findByRole('button', { name: 'Edit target IC 1805' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Target name' }), { target: { value: 'IC 1805 core' } });
    fireEvent.click(screen.getByRole('button', { name: 'Move to another project...' }));
    const dialog = await screen.findByRole('dialog', { name: 'Move target' });
    expect(targetUpdates).toEqual([{ name: 'IC 1805 core' }]);
    expect(within(dialog).getByText('IC 1805 core')).toBeInTheDocument();
    setDisplayPreferences({ showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' });
  });

  it('still renames a target from Edit target', async () => {
    const { targetUpdates } = renderLibrary();
    fireEvent.click(await screen.findByRole('button', { name: 'Edit target IC 1805' }));
    const name = screen.getByRole('textbox', { name: 'Target name' });
    fireEvent.change(name, { target: { value: 'IC 1805 core' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(targetUpdates).toEqual([{ name: 'IC 1805 core' }]));
    setDisplayPreferences({ showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' });
  });
});
