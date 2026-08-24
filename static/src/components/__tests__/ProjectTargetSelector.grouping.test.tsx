import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectTargetSelector from '../ProjectTargetSelector';
import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../../hooks/useDisplayPreferences';
import { renderHook } from '@testing-library/react';

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

function serveTwoCatalogs() {
  server.use(
    http.get('/api/databases', () =>
      ok([
        { id: 'attic', name: 'Attic catalog', path: '/attic.sqlite' },
        { id: 'shed', name: 'Shed catalog', path: '/shed.sqlite' },
      ])
    ),
    http.get('/api/db/attic/projects', () => ok([project(1, 'Sh2 86')])),
    http.get('/api/db/shed/projects', () => ok([project(1, 'NGC 6820')])),
    http.get('/api/db/:dbId/targets', () => ok([]))
  );
}

function renderSelector() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/grid']}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  }
  return render(<ProjectTargetSelector />, { wrapper: Wrapper });
}

async function openPicker() {
  const trigger = document.querySelector<HTMLButtonElement>('#scope-select')!;
  await waitFor(() => expect(trigger).not.toBeDisabled());
  fireEvent.click(trigger);
}

function groupHeadings(): string[] {
  return Array.from(
    document.querySelectorAll('.selector-group-heading > span:first-child')
  ).map((el) => el.textContent ?? '');
}

afterEach(() => {
  const { result } = renderHook(() => useDisplayPreferences());
  setDisplayPreferences({ ...result.current, projectPickerGrouping: 'activity' });
});

describe('the project picker grouping preference', () => {
  it('groups by database when the preference says so', async () => {
    serveTwoCatalogs();
    const { result } = renderHook(() => useDisplayPreferences());
    setDisplayPreferences({ ...result.current, projectPickerGrouping: 'database' });

    renderSelector();
    await openPicker();

    await screen.findByText('Sh2 86');
    expect(groupHeadings()).toEqual(['Attic catalog', 'Shed catalog']);
    const attic = document.querySelector('[aria-label="Attic catalog"]')!;
    expect(attic.textContent).toContain('Sh2 86');
    expect(attic.textContent).not.toContain('NGC 6820');
  });

  it('groups by activity by default, with no database headings', async () => {
    serveTwoCatalogs();
    renderSelector();
    await openPicker();

    await screen.findByText('Sh2 86');
    expect(groupHeadings()).not.toContain('Attic catalog');
    // Both catalogs' projects share one activity group instead.
    const headings = groupHeadings();
    expect(headings.length).toBeGreaterThan(0);
  });
});
