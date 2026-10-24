import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, waitFor, type RenderOptions } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import type { ReactElement, ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { expect } from 'vitest';
import type { StackScoringSettings } from '../api/types';
import { AccessContext, type AccessContextValue } from '../auth/access';
import { SCORING_DEFAULTS_QUERY_KEY } from '../hooks/useScoringPreferences';
import {
  SCORING_DEFAULTS,
  scoringPreferencesOf,
  stackScoringSettings,
  type ScoringPreferences,
} from '../utils/scoring';

/**
 * A stand-in for the server's scoring defaults (`/api/settings/scoring`),
 * served by the default MSW handlers. A test sets what the server holds
 * with `set`, reads what a save left with `get`, and finds every save in
 * `saves`. Setting `failure` makes saves fail. `afterEach` in setup.ts
 * resets it to the built-in values, stored.
 */
export const scoringServer = {
  value: { ...SCORING_DEFAULTS } as ScoringPreferences,
  stored: true,
  saves: [] as ScoringPreferences[],
  /** When set, a save fails this way and the server keeps what it had. */
  failure: null as null | { status: 500 } | { error: string },
  set(next: ScoringPreferences, stored = true) {
    this.value = { ...next };
    this.stored = stored;
  },
  get(): ScoringPreferences {
    return { ...this.value };
  },
  reset() {
    this.value = { ...SCORING_DEFAULTS };
    this.stored = true;
    this.saves = [];
    this.failure = null;
  },
};

const response = () => ({
  scoring: stackScoringSettings(scoringServer.value),
  builtin: stackScoringSettings(SCORING_DEFAULTS),
  stored: scoringServer.stored,
});

export const scoringDefaultsHandlers = [
  http.get('/api/settings/scoring', () =>
    HttpResponse.json({ success: true, data: response(), error: null }),
  ),
  http.put('/api/settings/scoring', async ({ request }) => {
    const failure = scoringServer.failure;
    if (failure && 'status' in failure) {
      return HttpResponse.json({ success: false, data: null, error: 'Internal error' }, { status: failure.status });
    }
    if (failure) return HttpResponse.json({ success: false, data: null, error: failure.error });
    const next = scoringPreferencesOf((await request.json()) as StackScoringSettings);
    scoringServer.set(next);
    scoringServer.saves.push(next);
    return HttpResponse.json({ success: true, data: response(), error: null });
  }),
];

/** A query client for one test: no retries, nothing cached between tests. */
export function testQueryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
}

/** Access for a person who may only look: a signed-in viewer. */
export const VIEWER_ACCESS: AccessContextValue = {
  status: {
    authentication_required: true,
    authenticated: true,
    role: 'read_only',
    can_compute: false,
  },
  canWrite: false,
  canCompute: false,
  logout: async () => undefined,
};

/** Render inside a query client and a router, and return the client so the
 *  test can deliver defaults saved elsewhere (`deliverScoringDefaults`).
 *  With `viewer`, the person may not change settings. */
export function renderWithScoring(
  ui: ReactElement,
  { route = '/', viewer = false, ...options }: RenderOptions & { route?: string; viewer?: boolean } = {},
) {
  const client = testQueryClient();
  const Wrapper = ({ children }: { children: ReactNode }) => {
    const routed = (
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
    return viewer ? <AccessContext.Provider value={VIEWER_ACCESS}>{routed}</AccessContext.Provider> : routed;
  };
  return { client, ...render(ui, { wrapper: Wrapper, ...options }) };
}

/** Wait until the page has the server's defaults, not the built-in values
 *  it shows until then. */
export async function waitForScoringDefaults(client: QueryClient): Promise<void> {
  await waitFor(() => expect(client.getQueryState(SCORING_DEFAULTS_QUERY_KEY)?.status).toBe('success'));
}

/** Defaults saved elsewhere (another person, another tab) reaching this
 *  page: the server holds them, and the page's query has them. */
export function deliverScoringDefaults(client: QueryClient, next: ScoringPreferences): void {
  scoringServer.set(next);
  client.setQueryData(SCORING_DEFAULTS_QUERY_KEY, response());
}
