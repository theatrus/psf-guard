import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import ObservingPreferences from '../director/ObservingPreferences';
import type { ObservingSettings } from '../../api/directorPreferences';

const policy = { weights: { importance: 30, window_urgency: 25, altitude: 15, moon_opportunity: 15, completion: 5, efficiency: 5, continuity: 5 }, importance: 50, minimum_dwell_ms: 600000, switch_margin: 500 };
const empty = (scope: ObservingSettings['scope'], scope_id: string): ObservingSettings => ({ scope, scope_id, revision: 0, enabled: null, site_id: null, overrides: { weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null } });
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
function mount(conflict = false) {
  const saved: ObservingSettings[] = [];
  const rig = { ...empty('rig', 'rig'), site_id: 'site' };
  server.use(
    http.get('/api/director/v1/rigs/profiles', () => ok([])),
    http.get('/api/director/v1/preferences', () => ok({ global_id: 'global', sites: [{ id: 'site', name: 'Mountain' }], presets: { balanced: policy, finish_goals: policy, best_conditions: policy } })),
    http.get('/api/director/v1/rigs/rig/preferences', () => ok({ enabled: true, resolved: { policy, provenance: {} }, settings: [empty('global', 'global'), empty('site', 'site'), rig, empty('project', 'project')] })),
    http.get('/api/director/v1/preferences/:scope/:id', ({ params }) => ok(params.scope === 'rig' ? rig : empty(params.scope as ObservingSettings['scope'], String(params.id)))),
    http.put('/api/director/v1/preferences/:scope/:id', async ({ request }) => { const body = await request.json() as ObservingSettings; saved.push(body); return conflict ? HttpResponse.json({ success: false, error: 'revision conflicts' }, { status: 409 }) : ok({ ...body, revision: body.revision + 1 }); }),
  );
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}><ObservingPreferences projectId="project" rigs={[{ id: 'rig', name: 'RedCat' }]} /></QueryClientProvider>);
  return saved;
}

describe('observing preference controls', () => {
  it('saves an explicit zero instead of inheriting it, with optimistic revision', async () => {
    const saved = mount();
    const input = await screen.findByLabelText(/Importance/, { selector: '#observing-importance' });
    expect(input).toBeDisabled();
    fireEvent.click(within(input.parentElement!).getByRole('checkbox'));
    fireEvent.change(input, { target: { value: '0' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save preferences' }));
    expect(await screen.findByText('Preferences saved.')).toBeInTheDocument();
    expect(saved[0].overrides.importance).toBe(0);
    expect(saved[0].revision).toBe(0);
    expect(saved[0].enabled).toBeNull();
  });
  it('supports rig opt-in and presets without changing project importance', async () => {
    const saved = mount();
    await screen.findByRole('button', { name: 'Save preferences' });
    fireEvent.change(screen.getByLabelText('Preference scope'), { target: { value: 'rig' } });
    const mode = await screen.findByLabelText('Scheduling mode');
    fireEvent.change(mode, { target: { value: 'preferences' } });
    fireEvent.change(screen.getByLabelText('Observing preset'), { target: { value: 'balanced' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save preferences' }));
    await screen.findByText('Preferences saved.');
    expect(saved[0].scope).toBe('rig');
    expect(saved[0].enabled).toBe(true);
    expect(saved[0].overrides.weights).toEqual(policy.weights);
    expect(saved[0].overrides.importance).toBeNull();
  });
  it('keeps a conflicted draft and allows explicit reload', async () => {
    mount(true);
    const input = await screen.findByLabelText(/Importance/, { selector: '#observing-importance' });
    fireEvent.click(within(input.parentElement!).getByRole('checkbox'));
    fireEvent.change(input, { target: { value: '80' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save preferences' }));
    await screen.findByRole('alert');
    expect(input).toHaveValue(80);
    fireEvent.click(screen.getByRole('button', { name: 'Reload saved preferences' }));
    await screen.findByDisplayValue('50');
    expect(input).toBeDisabled();
  });
});
