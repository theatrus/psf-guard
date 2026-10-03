import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, within, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import ObservingPreferences from '../director/ObservingPreferences';
import type { ObservingSettings } from '../../api/directorPreferences';

const empty = (scope: ObservingSettings['scope'], scope_id: string): ObservingSettings => ({ scope, scope_id, revision: 0, enabled: null, site_id: null, project_order: null, overrides: { weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null } });
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
function mount(conflict = false, rigs = [{ id: 'rig', name: 'RedCat' }]) {
  const saved: ObservingSettings[] = [];
  const rig = { ...empty('rig', 'rig'), site_id: 'site' };
  const global = { ...empty('global', 'global'), project_order: ['andromeda', 'orion'] };
  server.use(
    http.get('/api/director/v1/rigs/profiles', () => ok([])),
    http.get('/api/director/v1/preferences', () => ok({ global_id: 'global', sites: [{ id: 'site', name: 'Mountain' }], presets: {} })),
    http.get('/api/director/v1/rigs/rig/preferences', () => ok({ enabled: false, project_order: global.project_order, order_source: { scope: 'global' }, settings: [global, empty('site', 'site'), rig] })),
    http.get('/api/director/v1/preferences/:scope/:id', ({ params }) => ok(params.scope === 'rig' ? rig : params.scope === 'global' ? global : empty('site', String(params.id)))),
    http.put('/api/director/v1/preferences/:scope/:id', async ({ request }) => { const body = await request.json() as ObservingSettings; saved.push(body); return conflict ? HttpResponse.json({ success: false, error: 'revision conflicts' }, { status: 409 }) : ok({ ...body, revision: body.revision + 1 }); }),
  );
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}><ObservingPreferences projectId="orion" rigs={rigs} projects={[{ id: 'orion', name: 'Orion' }, { id: 'andromeda', name: 'Andromeda' }]} /></QueryClientProvider>);
  return saved;
}

describe('project priority controls', () => {
  it('saves a global ordered list without project weights', async () => {
    const saved = mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Move Orion up' }));
    expect(within(screen.getByRole('list', { name: 'Ranked projects' })).getAllByRole('listitem')[0]).toHaveTextContent('Orion');
    expect(screen.queryByRole('option', { name: 'Project override' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/Importance/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save priority' }));
    await screen.findByText('Project priority saved.');
    expect(saved[0]).toMatchObject({ scope: 'global', project_order: ['orion', 'andromeda'], revision: 0, overrides: empty('global', 'global').overrides });
  });
  it('inherits globally by default and allows an explicit rig override', async () => {
    const saved = mount();
    await screen.findByText('Following global order');
    fireEvent.change(screen.getByLabelText('Priority scope'), { target: { value: 'rig' } });
    const inherit = await screen.findByLabelText('Use inherited order');
    expect(inherit).toBeChecked();
    expect(screen.getByRole('button', { name: 'Move Orion up' })).toBeDisabled();
    fireEvent.click(inherit);
    fireEvent.click(screen.getByRole('button', { name: 'Move Orion up' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save priority' }));
    await screen.findByText('Project priority saved.');
    expect(saved[0]).toMatchObject({ scope: 'rig', site_id: 'site', project_order: ['orion', 'andromeda'] });
    fireEvent.click(inherit);
    fireEvent.click(screen.getByRole('button', { name: 'Save priority' }));
    await waitFor(() => expect(saved).toHaveLength(2));
    expect(saved[1].project_order).toBeNull();
  });
  it('keeps a conflicted draft and allows explicit reload', async () => {
    mount(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Move Orion up' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save priority' }));
    await screen.findByRole('alert');
    expect(screen.getByRole('button', { name: 'Move Orion up' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Reload saved priority' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Move Orion up' })).toBeEnabled());
  });
  it('updates the inherited order when the draft planning site changes', async () => {
    mount();
    server.use(http.get('/api/director/v1/preferences/site/site', () => ok({ ...empty('site', 'site'), project_order: ['orion', 'andromeda'] })));
    await screen.findByText('Following global order');
    fireEvent.change(screen.getByLabelText('Priority scope'), { target: { value: 'rig' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Move Orion down' })).toBeDisabled());
    const first = () => within(screen.getByRole('list', { name: 'Ranked projects' })).getAllByRole('listitem')[0];
    await waitFor(() => expect(first()).toHaveTextContent('Orion'));
    fireEvent.change(screen.getByLabelText('Planning site'), { target: { value: '' } });
    await waitFor(() => expect(first()).toHaveTextContent('Andromeda'));
  });
  it('allows editing the global order before any rig is linked', async () => {
    const saved = mount(false, []);
    fireEvent.click(await screen.findByRole('button', { name: 'Save priority' }));
    await screen.findByText('Project priority saved.');
    expect(saved[0].scope).toBe('global');
  });
});
