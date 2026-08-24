import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';

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
    description: null,
    has_files: true,
    state: 1,
    latest_image_date: 1_705_352_400,
  };
}

function target(id: number, projectId: number, name: string) {
  return { id, project_id: projectId, name, active: true, has_files: true };
}

function serveCatalog() {
  server.use(
    http.get('/api/databases', () =>
      ok([{ id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' }])
    ),
    http.get('/api/db/attic/projects', () =>
      ok([project(1, 'Sh2 86'), project(2, 'NGC 6820')])
    ),
    http.get('/api/db/attic/targets', () =>
      ok([target(4, 1, 'Sh2 86 East'), target(5, 2, 'NGC 6820 Core')])
    )
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

const originalScrollIntoView = Element.prototype.scrollIntoView;

afterEach(() => {
  Element.prototype.scrollIntoView = originalScrollIntoView;
});

async function openPicker() {
  const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
  await waitFor(() => expect(trigger).not.toBeDisabled());
  fireEvent.click(trigger);
}

describe('the open project picker', () => {
  it('lands on the selected target instead of the top of the list', async () => {
    serveCatalog();
    const scrolled: Element[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    renderAt('/grid?db=attic&project=1&target=4');

    await openPicker();

    await waitFor(() => expect(scrolled).toHaveLength(1));
    expect(scrolled[0]).toHaveAttribute('aria-current', 'true');
    expect(scrolled[0].textContent).toContain('Sh2 86 East');
  });

  it('scrolls nowhere when nothing is selected', async () => {
    serveCatalog();
    const scrolled: Element[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    renderAt('/grid');

    await openPicker();

    // Give the targets query time to land; the top "Choose a project" row is
    // marked current with no scope, and it needs no scrolling to be seen.
    await waitFor(() => expect(document.querySelector('.selector-options')).not.toBeNull());
    expect(scrolled.filter((el) => el.matches('.selector-option'))).toHaveLength(0);
  });
});
