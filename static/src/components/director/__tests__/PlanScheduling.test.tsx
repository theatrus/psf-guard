import type { ReactNode, MutableRefObject } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import { AccessContext, useAccess } from '../../../auth/access';
import PlanScheduling from '../PlanScheduling';
import { DraftProvider } from '../pageDrafts';
import { usePageDrafts } from '../pageDraftsState';
import type { ObservingSettings, SchedulingValues } from '../../../api/directorPreferences';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const project = 'pppppppp-pppp-4ppp-8ppp-pppppppppppp';
const redcat = { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat' };
const c925 = { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'C925' };
const ts: SchedulingValues = { minimum_time_minutes: 30, minimum_altitude_degrees: 0, maximum_altitude_degrees: 0, use_custom_horizon: false, horizon_offset_degrees: 0, meridian_window_minutes: 0, filter_switch_frequency: 0, dither_every: 0, smart_exposure_order: false };
const global = { scope: 'global' as const, id: 'g', revision: 2 };
const rigSource = { scope: 'rig' as const, id: c925.id, revision: 1 };

function fixture() {
  let stored: ObservingSettings = { scope: 'project', scope_id: project, revision: 0, overrides: { weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null }, enabled: null, site_id: null, project_order: null };
  const saves: ObservingSettings[] = [];
  const effective = (rig: string, withProject: boolean) => {
    const values = { ...ts, minimum_altitude_degrees: 25, meridian_window_minutes: rig === c925.id ? 30 : 20 };
    const sources: Record<string, unknown> = { minimum_altitude_degrees: global, meridian_window_minutes: rig === c925.id ? rigSource : global };
    const override = withProject ? stored.scheduling?.minimum_altitude_degrees : undefined;
    if (override !== undefined && override !== null) { values.minimum_altitude_degrees = override; sources.minimum_altitude_degrees = { scope: 'project', id: project, revision: stored.revision }; }
    return { enabled: true, project_order: null, order_source: null, settings: [], resolved: {}, scheduling: { values, sources } };
  };
  server.use(
    http.get(`/api/director/v1/preferences/project/${project}`, () => ok(stored)),
    http.put(`/api/director/v1/preferences/project/${project}`, async ({ request }) => {
      const body = await request.json() as ObservingSettings;
      saves.push(body);
      stored = { ...body, revision: body.revision + 1 };
      return ok(stored);
    }),
    http.get('/api/director/v1/rigs/:rig/preferences', ({ params, request }) => ok(effective(String(params.rig), new URL(request.url).searchParams.has('project_id')))),
  );
  return { saves };
}

type Drafts = ReturnType<typeof usePageDrafts>;
function OnPage({ out }: { out: MutableRefObject<Drafts | null> }) {
  const drafts = usePageDrafts();
  out.current = drafts;
  return <DraftProvider drafts={drafts}><PlanScheduling projectId={project} rigs={[redcat, c925]} /></DraftProvider>;
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  const drafts: MutableRefObject<Drafts | null> = { current: null };
  render(<OnPage out={drafts} />, { wrapper: Wrapper });
  return drafts;
}

describe('plan scheduling limits', () => {
  it('shows what each rig gets and where each value comes from', async () => {
    fixture();
    mount();
    const table = await screen.findByRole('table');
    const row = (label: string) => within(table).getByRole('row', { name: new RegExp(`^${label}`) });
    await waitFor(() => expect(row('Minimum altitude')).toHaveTextContent('25°from every plan25°from every plan'));
    expect(row('Meridian window')).toHaveTextContent('20 minfrom every plan30 minfrom rig');
    expect(row('Minimum time')).toHaveTextContent('30 minTarget Scheduler default');
    // An empty field says what it inherits.
    expect(screen.getByRole('spinbutton', { name: 'Minimum time' })).toHaveAttribute('placeholder', '30 min');
  });

  it('saves an override through the page bar and can return to inheriting', async () => {
    const { saves } = fixture();
    const drafts = mount();
    const altitude = await screen.findByRole('spinbutton', { name: 'Minimum altitude' });
    fireEvent.change(altitude, { target: { value: '35' } });
    await waitFor(() => expect(drafts.current!.unsaved.map(section => section.label)).toEqual(['Scheduling limits']));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[0].scheduling).toEqual({ minimum_altitude_degrees: 35 });
    await waitFor(() => expect(within(screen.getByRole('table')).getByRole('row', { name: /^Minimum altitude/ })).toHaveTextContent('35°from this plan'));
    expect(screen.getByText('set here')).toBeInTheDocument();
    // Its own save is no news from elsewhere.
    expect(screen.queryByText(/changed elsewhere/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Inherit minimum altitude' }));
    expect(await drafts.current!.saveAll()).toBeNull();
    expect(saves[1].scheduling).toEqual({});
  });

  it('keeps edits through a conflict, loads the newer limits under them, and reloads on request', async () => {
    const { saves } = fixture();
    const drafts = mount();
    const altitude = await screen.findByRole('spinbutton', { name: 'Minimum altitude' });
    fireEvent.change(altitude, { target: { value: '35' } });
    await waitFor(() => expect(drafts.current!.unsaved).toHaveLength(1));
    // Someone else saved other limits first.
    const theirs = { scope: 'project', scope_id: project, revision: 1, overrides: { weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null }, enabled: null, site_id: null, project_order: null, scheduling: { minimum_time_minutes: 45 } };
    server.use(
      http.put(`/api/director/v1/preferences/project/${project}`, () => HttpResponse.json({ success: false, data: null, error: 'Conflict' }, { status: 409 })),
      http.get(`/api/director/v1/preferences/project/${project}`, () => ok(theirs)),
    );
    expect(await drafts.current!.saveAll()).toMatchObject({ label: 'Scheduling limits' });
    expect(await screen.findByText(/These limits changed elsewhere/)).toBeInTheDocument();
    expect(screen.getByRole('spinbutton', { name: 'Minimum altitude' })).toHaveValue(35);
    // A second save would undo theirs unseen; it waits for a reload.
    expect(await drafts.current!.saveAll()).toEqual({ label: 'Scheduling limits', reason: 'the limits changed elsewhere; reload them first' });
    expect(saves).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    await waitFor(() => expect(screen.getByRole('spinbutton', { name: 'Minimum time' })).toHaveValue(45));
    expect(screen.getByRole('spinbutton', { name: 'Minimum altitude' })).toHaveValue(null);
    expect(screen.queryByText(/These limits changed elsewhere/)).not.toBeInTheDocument();
  });

  it('is read only without write access', async () => {
    fixture();
    mount(false);
    expect(await screen.findByRole('spinbutton', { name: 'Minimum altitude' })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'Custom horizon' })).toBeDisabled();
  });
});
