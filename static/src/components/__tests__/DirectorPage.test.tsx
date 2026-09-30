import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import { useScopedDbId } from '../../hooks/useUrlState';
import DirectorPage from '../director/DirectorPage';
import type { DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false, database_management: true };
const row = (project = record, extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project, links: [], progress: null, framing: null, plan: null, activation: null, ...extra });
const list = (rows: DirectorPlanRow[], warnings: string[] = []) => ({ rows, warnings });
/** The display preference module reads storage once and again on a storage
 *  event, so set the density the way another tab would. */
function chooseDensity(density: 'compact' | 'detailed' | null) {
  if (density) localStorage.setItem('psf-guard.display-preferences', JSON.stringify({ libraryDensity: density }));
  else localStorage.removeItem('psf-guard.display-preferences');
  window.dispatchEvent(new StorageEvent('storage', { key: 'psf-guard.display-preferences' }));
}
function Location() {
  const location = useLocation();
  const db = useScopedDbId();
  return <output data-testid="location">{location.search}|scope={db ?? 'global'}</output>;
}
/** A test's own handlers go first: msw matches the first handler it was given. */
function mount(canWrite = true, route = '/director?db=old-catalog&project=123', handlers: Parameters<typeof server.use> = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></AccessContext.Provider></QueryClientProvider>;
  }
  server.use(
    ...handlers,
    http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'c925', name: 'C925' }]))),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([]))),
    http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([]))),
    // Plan thumbnails: a survey image; the panels are drawn from the framing itself.
    http.get('/api/director/v1/sky/cutout', () => HttpResponse.arrayBuffer(new Uint8Array([255, 216, 255]).buffer, { status: 200, headers: { 'content-type': 'image/jpeg' } })),
  );
  render(<DirectorPage />, { wrapper: Wrapper });
}

describe('Planning page', () => {
  afterEach(() => { vi.unstubAllGlobals(); chooseDensity(null); });

  it('sends the old plan list to the Library with its scope, Show and search', async () => {
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([])))));
    mount(true, '/director?db=old-catalog&project=123&directorView=sites&directorShow=done&directorSearch=m3');
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123&show=done&q=m3|scope=global'));
    expect(screen.queryByRole('heading', { name: 'Plans' })).not.toBeInTheDocument();
  });

  it('keeps a plan workspace on the page', async () => {
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([row()])))));
    mount(true, `/director?db=old-catalog&project=123&directorProject=${record.id}`);
    expect(await screen.findByRole('heading', { name: 'M31' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Library' })).toHaveAttribute('href', '/?db=old-catalog&project=123');
  });

  it.each([{ ...enabled, enabled: false }, { ...enabled, protocol_version: 2 }])('does not load records when unavailable: %j', async status => {
    const plansCall = vi.fn(() => HttpResponse.json(ok(list([]))));
    mount();
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))), http.get('/api/director/v1/plans', plansCall));
    expect(await screen.findByText('Planning is unavailable on this server.')).toBeInTheDocument();
    expect(plansCall).not.toHaveBeenCalled();
  });
});
