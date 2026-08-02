import { describe, expect, it } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

const databases = [
  { id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' },
  { id: 'shed', name: 'Shed catalog', path: '/shed.sqlite' },
];

function project(id: number, name: string) {
  return {
    id,
    profile_id: 'profile',
    profile_name: 'Profile',
    name,
    display_name: name,
    description: null,
    has_files: true,
    state: 1,
    latest_image_date: 1_705_352_400,
  };
}

function target(id: number, projectId: number, name: string) {
  return { id, project_id: projectId, name, active: true, has_files: true };
}

/**
 * Both catalogs carry a project of the same name, which is why the closed
 * trigger has to say which one is in view.
 */
function serveTwoCatalogs() {
  server.use(
    http.get('/api/databases', () => ok(databases)),
    http.get('/api/db/attic/projects', () => ok([project(1, '2026 - Sh2 86')])),
    http.get('/api/db/shed/projects', () => ok([project(1, '2026 - Sh2 86')])),
    http.get('/api/db/attic/targets', () => ok([target(4, 1, 'Sh2 86')])),
    http.get('/api/db/shed/targets', () => ok([]))
  );
}

function renderAt(route: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  }
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

function trigger() {
  return document.querySelector('#scope-select')!;
}

function scopeLines() {
  const scope = trigger().querySelector('.selector-trigger-scope')!;
  return {
    name: scope.querySelector('span')?.textContent,
    database: scope.querySelector('small')?.textContent ?? null,
  };
}

describe('the closed project picker', () => {
  it('names the database under the project', async () => {
    serveTwoCatalogs();
    renderAt('/grid?db=shed&project=1');

    await waitFor(() =>
      expect(scopeLines()).toEqual({ name: '2026 - Sh2 86', database: 'Shed catalog' })
    );
  });

  it('keeps naming the database once a target narrows the view', async () => {
    serveTwoCatalogs();
    renderAt('/grid?db=attic&project=1&target=4');

    await waitFor(() =>
      expect(scopeLines()).toEqual({
        name: '2026 - Sh2 86 · Sh2 86',
        database: 'Attic catalog',
      })
    );
  });

  it('names the database on its own line when no project is chosen yet', async () => {
    serveTwoCatalogs();
    renderAt('/grid?db=attic');

    await waitFor(() =>
      expect(scopeLines()).toEqual({ name: 'All projects', database: 'Attic catalog' })
    );
  });

  it('says nothing about a database when nothing is in scope', async () => {
    serveTwoCatalogs();
    renderAt('/grid');

    await waitFor(() =>
      expect(scopeLines()).toEqual({ name: 'Choose project or target', database: null })
    );
  });
});
