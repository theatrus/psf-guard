import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import { useScopedDbId } from '../../hooks/useUrlState';
import DirectorPage from '../director/DirectorPage';

const ok = (data: unknown) => ({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false };
function Location() {
  const location = useLocation();
  const db = useScopedDbId();
  return <output data-testid="location">{location.search}|scope={db ?? 'global'}</output>;
}
function mount(canWrite = true, route = '/director?db=old-catalog&project=123') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></AccessContext.Provider></QueryClientProvider>;
  }
  server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))));
  render(<DirectorPage />, { wrapper: Wrapper });
}

describe('Director management', () => {
  it('creates with the same UUID after an ambiguous failure, and trims names', async () => {
    const requests: { id: string; name: string }[] = [];
    let committed = false;
    server.use(
      http.get('/api/director/v1/projects', () => HttpResponse.json(ok({ items: committed ? [{ ...record, ...requests[0] }] : [], next_after: null }))),
      http.post('/api/director/v1/projects', async ({ request }) => {
        requests.push(await request.json() as { id: string; name: string });
        committed = true;
        return requests.length === 1 ? HttpResponse.error() : HttpResponse.json(ok({ ...requests[0], revision: 1 }));
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'New project' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: '  Sky survey  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Network Error');
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Created Sky survey.')).toBeInTheDocument();
    expect(requests).toHaveLength(2);
    expect(requests[0]).toEqual(requests[1]);
    expect(requests[0].name).toBe('Sky survey');
    expect(requests[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });

  it('keeps a stale rename draft until canceled and reloads the current revision', async () => {
    let current = record;
    const updates: unknown[] = [];
    server.use(
      http.get('/api/director/v1/projects', () => HttpResponse.json(ok({ items: [current], next_after: null }))),
      http.patch('/api/director/v1/projects/:id', async ({ request, params }) => {
        expect(params.id).toBe(record.id);
        const body = await request.json() as { expected_revision: number; name: string };
        updates.push(body);
        if (body.expected_revision !== current.revision) return HttpResponse.json({ error: 'Revision conflict; reload before retrying' }, { status: 409 });
        current = { ...current, name: body.name, revision: current.revision + 1 };
        return HttpResponse.json(ok(current));
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Rename M31' }));
    current = { ...record, name: 'Changed elsewhere', revision: 2 };
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Revision conflict');
    expect(screen.getByLabelText('Project name')).toHaveValue('Andromeda');
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh records' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Rename Changed elsewhere' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Renamed Andromeda.')).toBeInTheDocument();
    expect(updates).toEqual([{ name: 'Andromeda', expected_revision: 1 }, { name: 'Andromeda', expected_revision: 2 }]);
  });

  it('supports read-only paging without write controls', async () => {
    server.use(http.get('/api/director/v1/projects', ({ request }) => {
      const params = new URL(request.url).searchParams;
      expect(params.get('limit')).toBe('64');
      return HttpResponse.json(ok(params.has('after')
        ? { items: [{ ...record, id: '22222222-2222-4222-8222-222222222222', name: 'M42' }], next_after: null }
        : { items: [record], next_after: record.id }));
    }));
    mount(false);
    expect(await screen.findByText('M31')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /New|Rename/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));
    expect(await screen.findByText('M42')).toBeInTheDocument();
    expect(screen.getByText('M31')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
  });

  it('separates site and rig collections while retaining catalog URL state', async () => {
    server.use(
      http.get('/api/director/v1/sites', () => HttpResponse.json(ok({ items: [{ ...record, name: 'Remote observatory' }], next_after: null }))),
      http.get('/api/director/v1/rigs', () => HttpResponse.json(ok({ items: [{ ...record, name: 'C925' }], next_after: null }))),
    );
    mount(true, '/director?db=old-catalog&project=123&directorView=sites');
    expect(await screen.findByText('Remote observatory')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Rigs' }));
    expect(await screen.findByText('C925')).toBeInTheDocument();
    expect(screen.queryByText('Remote observatory')).not.toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123&directorView=rigs|scope=global');
  });

  it.each([{ ...enabled, enabled: false }, { ...enabled, protocol_version: 2 }])('does not load records when unavailable: %j', async status => {
    const list = vi.fn(() => HttpResponse.json(ok({ items: [], next_after: null })));
    mount();
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))), http.get('/api/director/v1/projects', list));
    expect(await screen.findByText('Director management is unavailable on this server.')).toBeInTheDocument();
    expect(list).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'New project' })).not.toBeInTheDocument();
  });

  it('rejects overlong multibyte names before sending them', async () => {
    const create = vi.fn(() => HttpResponse.json(ok(record)));
    server.use(http.get('/api/director/v1/projects', () => HttpResponse.json(ok({ items: [], next_after: null }))), http.post('/api/director/v1/projects', create));
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'New project' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: '\u00e9'.repeat(300) } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('512 UTF-8 bytes');
    expect(create).not.toHaveBeenCalled();
  });

  it('recovers a failed initial listing without leaving a false empty state', async () => {
    let fail = true;
    server.use(http.get('/api/director/v1/projects', () => fail
      ? HttpResponse.json({ error: 'Director metadata is busy; retry shortly' }, { status: 503 })
      : HttpResponse.json(ok({ items: [record], next_after: null }))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('busy');
    expect(screen.queryByText('No projects yet.')).not.toBeInTheDocument();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh records' }));
    await waitFor(() => expect(screen.getByText('M31')).toBeInTheDocument());
  });
});
