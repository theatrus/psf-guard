import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import RigTargetSelect from '../RigTargetSelect';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function project(id: number, name: string, guid: string | null) {
  return { id, profile_id: 'p', profile_name: 'p', name, display_name: name, description: null, has_files: true, state: 1, latest_image_date: null, guid };
}

/** Askar holds M 44 with two targets; with `family`, Redcat holds the same
 *  project (one GUID) with one target of its own. */
function serve({ family }: { family: boolean }) {
  const databases = [
    { id: 'askar', name: 'Askar 107', database_path: '/askar.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ...(family ? [{ id: 'redcat', name: 'Redcat 51', database_path: '/redcat.sqlite', image_directories: [], remote_image_upload: { enabled: false } }] : []),
  ];
  server.use(
    http.get('/api/databases', () => ok(databases)),
    http.get('/api/db/askar/projects', () => ok([project(7, 'M 44', 'g44')])),
    http.get('/api/db/redcat/projects', () => ok([project(9, 'M 44', 'G44 ')])),
    http.get('/api/db/askar/targets', () => ok([
      { id: 3, project_id: 7, name: 'M 44 core', active: true, has_files: true },
      { id: 4, project_id: 7, name: 'M 44 halo', active: true, has_files: true },
    ])),
    http.get('/api/db/redcat/targets', () => ok([{ id: 5, project_id: 9, name: 'M 44', active: true, has_files: true }])),
    http.get('/api/db/:db/projects/:project/mosaic', () => ok({ mosaic: null })),
  );
}

function Location() {
  const location = useLocation();
  return <output aria-label="location">{`${location.pathname}${location.search}`}</output>;
}

function Go({ to }: { to: string }) {
  const navigate = useNavigate();
  const { search } = useLocation();
  return <button type="button" onClick={() => navigate(`${to}${search}`)}>{`Go ${to}`}</button>;
}

function renderAt(entry: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const page = (
    <>
      <RigTargetSelect />
      <Location />
      <Go to="/grid" />
      <Go to="/calibration" />
    </>
  );
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>{children}</MemoryRouter>
    </QueryClientProvider>
  );
  return render(
    <Routes>
      <Route path="/calibration" element={page} />
      <Route path="/grid" element={page} />
    </Routes>,
    { wrapper },
  );
}

const location = () => new URL(`http://x${screen.getByRole('status', { name: 'location' }).textContent}`);

describe('RigTargetSelect on Calibration', () => {
  it('names the rig only, and gives the target back on Images', async () => {
    serve({ family: false });
    renderAt('/calibration?db=askar&project=7&target=3');
    const rig = await screen.findByLabelText('Rig: Askar 107');
    expect(rig).toHaveTextContent('Askar 107');
    expect(rig).not.toHaveTextContent('M 44');
    expect(screen.queryByRole('combobox', { name: 'Rig and target' })).not.toBeInTheDocument();
    // The target stays in the address for the views after it.
    expect(location().searchParams.get('target')).toBe('3');

    fireEvent.click(screen.getByRole('button', { name: 'Go /grid' }));
    const picker = await screen.findByRole('combobox', { name: 'Rig and target' });
    await waitFor(() => expect(picker).toHaveDisplayValue(/core/));
  });

  it('offers only rigs for a project shot by two, and opens the same project on the one chosen', async () => {
    serve({ family: true });
    renderAt('/calibration?db=askar&project=7&target=3');
    const rig = await screen.findByRole('combobox', { name: 'Rig' });
    await waitFor(() => expect(rig).toHaveDisplayValue('Askar 107'));
    const options = Array.from((rig as HTMLSelectElement).options).map((option) => option.text);
    expect(options).toEqual(['Askar 107', 'Redcat 51']);

    fireEvent.change(rig, { target: { value: 'redcat:9' } });
    await waitFor(() => expect(location().searchParams.get('db')).toBe('redcat'));
    expect(location().pathname).toBe('/calibration');
    expect(location().searchParams.get('project')).toBe('9');
    expect(location().searchParams.has('target')).toBe(false);

    // Back on Images, the picker reads rig and target again.
    fireEvent.click(screen.getByRole('button', { name: 'Go /grid' }));
    expect(await screen.findByRole('combobox', { name: 'Rig and target' })).toBeInTheDocument();
  });
});
