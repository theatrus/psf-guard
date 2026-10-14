import { afterEach, describe, expect, it } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { PROJECT_SEEN_STORAGE_KEY, projectSeenKey } from '../../utils/projectRecency';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

const now = Math.floor(Date.now() / 1000);

function project(id: number, name: string, latest: number) {
  return {
    id, guid: null, profile_id: 'profile', profile_name: 'Profile', name, display_name: name, has_files: true, state: 1, target_count: 1,
    total_images: 6, accepted_images: 1, rejected_images: 2, pending_images: 3, total_desired: 10, files_found: 6, files_missing: 0,
    date_range: { earliest: latest - 86_400, latest }, filters_used: ['Ha'], recent_images: [],
  };
}

function wrapper({ children }: { children: ReactNode }) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return <QueryClientProvider client={queryClient}><MemoryRouter initialEntries={['/']}>{children}</MemoryRouter></QueryClientProvider>;
}

const stored = () => JSON.parse(window.localStorage.getItem(PROJECT_SEEN_STORAGE_KEY) ?? '{}');

describe('the Library\'s first-sight markers', () => {
  afterEach(() => window.localStorage.removeItem(PROJECT_SEEN_STORAGE_KEY));

  it('seeds a project it has not seen, without writing over a marker stored since it opened', async () => {
    let answer: () => void = () => {};
    const answered = new Promise<void>((resolve) => { answer = resolve; });
    server.use(
      http.get('/api/databases', () => ok([{ id: 'rig', name: 'Rig', path: '/rig.sqlite' }])),
      http.get('/api/db/:dbId/projects/overview', async () => {
        await answered;
        return ok([project(1, 'Heart', now - 3_600), project(2, 'Soul', now - 7_200)]);
      }),
      http.get('/api/db/:dbId/targets/overview', () => ok([])),
      http.get('/api/settings/export', () => ok({ default_layout: 'flat' })),
    );
    render(<Overview />, { wrapper });

    // While the projects load, another tab (or a test) records that this
    // browser last saw Heart a day ago.
    const heart = projectSeenKey('rig', 1);
    window.localStorage.setItem(PROJECT_SEEN_STORAGE_KEY, JSON.stringify({
      [heart]: { latestImage: now - 86_400, totalImages: 2, targets: {} },
    }));
    answer();

    // Soul, never seen, gets its baseline; Heart keeps the one stored.
    await waitFor(() => expect(stored()).toHaveProperty(projectSeenKey('rig', 2)));
    expect(stored()[projectSeenKey('rig', 2)].latestImage).toBe(now - 7_200);
    expect(stored()[heart].latestImage).toBe(now - 86_400);
  });
});
