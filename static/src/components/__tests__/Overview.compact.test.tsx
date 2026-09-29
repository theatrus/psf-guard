import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { ago, percentDone, projectFamilies, stateLabel } from '../libraryFamilies';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function project(id: number, name: string, guid: string | null, accepted: number, desired: number, latest: number) {
  return {
    id, guid, profile_id: 'profile', profile_name: 'Profile', name, display_name: name, has_files: true, state: 1, target_count: 1,
    total_images: accepted + 5, accepted_images: accepted, rejected_images: 2, pending_images: 3, total_desired: desired, files_found: accepted + 5, files_missing: 0,
    date_range: { earliest: latest - 86_400 * 3, latest }, filters_used: ['Ha'], recent_images: [],
  };
}

function wrapper(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}><MemoryRouter initialEntries={[route]}>{children}</MemoryRouter></QueryClientProvider>;
  };
}

const now = Math.floor(Date.now() / 1000);
const shared = 'AAAAAAAA-0000-4000-8000-000000000001';

function twoRigs() {
  // Both databases answer with the same rows, so the shared GUID appears in each: one plan, two rigs.
  server.use(
    http.get('/api/databases', () => ok([{ id: 'redcat', name: 'RedCat', path: '/a.sqlite' }, { id: 'c925', name: 'C925', path: '/b.sqlite' }])),
    http.get('/api/db/:dbId/projects/overview', () => ok([project(1, 'Heart Nebula', shared, 10, 40, now - 3_600 * 5), project(2, 'Loner', null, 3, 0, now - 86_400 * 2)])),
    http.get('/api/db/:dbId/targets/overview', () => ok([])),
    http.get('/api/db/:dbId/stats/overall', () => ok({ total_projects: 2, active_projects: 2, total_targets: 2, active_targets: 2, total_images: 40, accepted_images: 26, rejected_images: 4, pending_images: 6, total_desired: 80, files_found: 40, files_missing: 0, unique_filters: ['Ha'], date_range: { earliest: now - 86_400 * 3, latest: now - 3_600 * 5 }, recent_activity: [] })),
    http.get('/api/settings/export', () => ok({ default_layout: 'flat' })),
  );
}

describe('compact Library', () => {
  it('shows one row of pills per project, wraps a plan shot by two rigs in an outer pill, and opens the full card on request', async () => {
    window.localStorage.removeItem('psf-guard.library.density');
    twoRigs();
    render(<Overview />, { wrapper: wrapper('/') });
    const family = await screen.findByTestId('library-family');
    // The outer pill sums both rigs: 20 of 80 desired, and names the databases.
    expect(within(family).getByText('2 rigs')).toBeInTheDocument();
    expect(within(family).getByText('20 / 80 · 25%')).toBeInTheDocument();
    expect(within(family).getByText(/^(RedCat · C925|C925 · RedCat)$/)).toBeInTheDocument();
    expect(within(family).getAllByTestId('library-row')).toHaveLength(2);
    // Each row: name, database, state, progress, grading and dates as pills; no card body.
    const rows = screen.getAllByTestId('library-row');
    expect(rows).toHaveLength(4);
    const heart = rows[0];
    expect(within(heart).getByRole('button', { name: 'Open Heart Nebula image grid' })).toBeInTheDocument();
    expect(within(heart).getByText('Active')).toBeInTheDocument();
    expect(within(heart).getByText('10 / 40 · 25%')).toBeInTheDocument();
    expect(within(heart).getByTitle('10 accepted, 2 rejected, 3 pending')).toBeInTheDocument();
    expect(within(heart).getByText(/5 h ago/)).toBeInTheDocument();
    expect(document.querySelector('.project-card')).toBeNull();
    // A project without a goal says how many images it has instead.
    const loner = screen.getAllByTestId('library-row').find(row => within(row).queryByText('Loner'))!;
    expect(within(loner).getByText('8 images')).toBeInTheDocument();
    // Opening one row shows the whole card for that project only, and Less folds it away.
    fireEvent.click(within(heart).getByRole('button', { name: 'Show details for Heart Nebula' }));
    const card = document.querySelector('.project-card')!;
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText('Desired progress')).toBeInTheDocument();
    expect(screen.getAllByTestId('library-row')).toHaveLength(3);
    fireEvent.click(within(card as HTMLElement).getByRole('button', { name: 'Hide details for Heart Nebula' }));
    expect(document.querySelector('.project-card')).toBeNull();
    // Detailed brings every card back, and the choice is remembered.
    fireEvent.click(screen.getByRole('radio', { name: 'Detailed' }));
    expect(document.querySelectorAll('.project-card')).toHaveLength(4);
    expect(screen.queryByTestId('library-row')).toBeNull();
    expect(window.localStorage.getItem('psf-guard.library.density')).toBe('detailed');
    window.localStorage.removeItem('psf-guard.library.density');
  });

  it('groups by GUID, sums progress, and names states', () => {
    const a = { ...project(1, 'Heart', shared, 10, 40, 100), db_id: 'a', db_name: 'A' };
    const b = { ...project(2, 'Heart', shared.toLowerCase(), 5, 20, 200), db_id: 'b', db_name: 'B' };
    const c = { ...project(3, 'Loner', null, 1, 0, 50), db_id: 'a', db_name: 'A' };
    const families = projectFamilies([a, c, b]);
    expect(families.map(f => f.members.length)).toEqual([2, 1]);
    expect(families[0]).toMatchObject({ name: 'Heart', accepted: 15, desired: 60, latest: 200 });
    expect(percentDone(15, 60)).toBe(25);
    expect(percentDone(3, 0)).toBeNull();
    expect(ago(null, 0)).toBeNull();
    expect(ago(1000, 1000 * 1000 + 30 * 60 * 1000)).toBe('just now');
    expect(ago(1000, 1000 * 1000 + 5 * 3600 * 1000)).toBe('5 h ago');
    expect(ago(1000, 1000 * 1000 + 3 * 86_400 * 1000)).toBe('3 d ago');
    expect([0, 1, 2, 3, 9].map(stateLabel)).toEqual(['Draft', 'Active', 'Inactive', 'Closed', 'State 9']);
  });
});
