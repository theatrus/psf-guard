import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  total_projects: 1,
  active_projects: 1,
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
};

function wrapper(route = '/') {
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

function useTwoCatalogs() {
  server.use(
    http.get('/api/databases', () =>
      ok([
        { id: 'alpha', name: 'Alpha catalog', path: '/alpha.sqlite' },
        { id: 'beta', name: 'Beta catalog', path: '/beta.sqlite' },
      ])
    ),
    http.get('/api/db/:dbId/projects/overview', ({ params }) =>
      ok(params.dbId === 'alpha' ? [project(1, 'Sh2 86')] : [project(1, 'NGC 6820')])
    ),
    http.get('/api/db/:dbId/targets/overview', () => ok([])),
    http.get('/api/db/:dbId/stats/overall', () => ok(stats)),
    http.get('/api/settings/export', () =>
      ok({ default_layout: 'wbpp' })
    )
  );
}

describe('Overview database filter', () => {
  it('narrows the projects list to the chosen database', async () => {
    useTwoCatalogs();
    render(<Overview />, { wrapper: wrapper() });

    await screen.findByText('Sh2 86');
    await screen.findByText('NGC 6820');

    const filter = screen.getByLabelText('Filter projects by database');
    fireEvent.change(filter, { target: { value: 'beta' } });

    await waitFor(() => expect(screen.queryByText('Sh2 86')).toBeNull());
    expect(screen.getByText('NGC 6820')).toBeInTheDocument();

    fireEvent.change(filter, { target: { value: 'all' } });
    await screen.findByText('Sh2 86');
  });

  it('offers no filter with a single database', async () => {
    server.use(
      http.get('/api/databases', () =>
        ok([{ id: 'solo', name: 'Only catalog', path: '/solo.sqlite' }])
      ),
      http.get('/api/db/:dbId/projects/overview', () => ok([project(1, 'Sh2 86')])),
      http.get('/api/db/:dbId/targets/overview', () => ok([])),
      http.get('/api/db/:dbId/stats/overall', () => ok(stats))
    );
    render(<Overview />, { wrapper: wrapper() });
    await screen.findByText('Sh2 86');
    expect(screen.queryByLabelText('Filter projects by database')).toBeNull();
  });
});

describe('Overview export dialog', () => {
  it('asks for the layout at export time, seeded from the settings default', async () => {
    useTwoCatalogs();
    render(<Overview />, { wrapper: wrapper() });

    const card = (await screen.findByText('Sh2 86')).closest('.project-card')!;
    const exportLink = await screen.findAllByText('⬇ Export');
    const alphaExport = exportLink.find((el) => card.contains(el))!;
    fireEvent.click(alphaExport);

    // The dialog offers both layouts and starts from the configured default.
    const dialog = await screen.findByRole('dialog', { name: 'Export Sh2 86' });
    expect(dialog).toBeInTheDocument();
    const wbpp = screen.getByRole('radio', { name: /WBPP/ });
    expect(wbpp).toBeChecked();

    // The chosen layout lands in the download URL.
    const download = screen.getByRole('link', { name: 'Download zip' });
    expect(download.getAttribute('href')).toContain('layout=wbpp');

    fireEvent.click(screen.getByRole('radio', { name: /Grouped by target/ }));
    expect(
      screen.getByRole('link', { name: 'Download zip' }).getAttribute('href')
    ).not.toContain('layout=');
  });
});
