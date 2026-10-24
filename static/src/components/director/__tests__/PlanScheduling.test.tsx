import { Profiler, type ReactNode, type MutableRefObject } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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

function fixture(scheduling?: ObservingSettings['scheduling']) {
  let stored: ObservingSettings = { scope: 'project', scope_id: project, revision: 0, overrides: { weights: {}, importance: null, minimum_dwell_ms: null, switch_margin: null }, enabled: null, site_id: null, project_order: null, scheduling };
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
  return { saves, stored: () => stored, store: (next: ObservingSettings) => { stored = next; } };
}

type Drafts = ReturnType<typeof usePageDrafts>;
function OnPage({ out }: { out: MutableRefObject<Drafts | null> }) {
  const drafts = usePageDrafts();
  out.current = drafts;
  return <DraftProvider drafts={drafts}><PlanScheduling projectId={project} rigs={[redcat, c925]} /></DraftProvider>;
}
function mount(canWrite = true, client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }), onRender?: (drafts: Drafts | null) => void) {
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  const drafts: MutableRefObject<Drafts | null> = { current: null };
  render(onRender ? <Profiler id="plan-scheduling" onRender={() => onRender(drafts.current)}><OnPage out={drafts} /></Profiler> : <OnPage out={drafts} />, { wrapper: Wrapper });
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
    let releaseLatest!: () => void;
    const latest = new Promise<void>(resolve => { releaseLatest = resolve; });
    let saveAttempts = 0;
    server.use(
      http.put(`/api/director/v1/preferences/project/${project}`, () => {
        saveAttempts += 1;
        return HttpResponse.json({ success: false, data: null, error: 'Conflict' }, { status: 409 });
      }),
      http.get(`/api/director/v1/preferences/project/${project}`, async () => { await latest; return ok(theirs); }),
    );
    await act(async () => {
      expect(await drafts.current!.saveAll()).toMatchObject({ label: 'Scheduling limits' });
    });
    expect(await screen.findByText(/These limits changed elsewhere/)).toBeInTheDocument();
    expect(screen.getByRole('spinbutton', { name: 'Minimum altitude' })).toHaveValue(35);
    // Block another save even before the newer limits have arrived.
    try {
      await act(async () => {
        expect(await drafts.current!.saveAll()).toEqual({ label: 'Scheduling limits', reason: 'the limits changed elsewhere; reload them first' });
      });
      expect(saveAttempts).toBe(1);
    } finally {
      releaseLatest();
    }
    expect(saves).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    await waitFor(() => expect(screen.getByRole('spinbutton', { name: 'Minimum time' })).toHaveValue(45));
    expect(screen.getByRole('spinbutton', { name: 'Minimum altitude' })).toHaveValue(null);
    await waitFor(() => expect(screen.queryByText(/These limits changed elsewhere/)).not.toBeInTheDocument());
  });

  it('shows the saved limits in the first render with them, with nothing unsaved', () => {
    const { stored } = fixture({ minimum_altitude_degrees: 35, meridian_window_minutes: 10 });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    client.setQueryData(['observingSettings', 'project', project], stored());
    // Read the DOM and the save bar's list at every commit: a render that
    // shows empty limits, or marks them unsaved, is what a test or a person
    // can catch between two commits.
    const shown: string[] = [];
    const record = (drafts: Drafts | null) => {
      const fields = ['Minimum altitude', 'Meridian window'].map(label => screen.queryByRole('spinbutton', { name: label }) as HTMLInputElement | null);
      if (fields.every(field => field)) shown.push(`${fields.map(field => field!.value).join(' ')} ${drafts?.unsaved.length ?? 0}`);
    };
    mount(true, client, record);
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.filter(values => values !== '35 10 0')).toEqual([]);
  });

  it('a newer copy replaces unedited limits, and flags edited ones', async () => {
    const { stored, store } = fixture({ minimum_altitude_degrees: 35 });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const drafts = mount(true, client);
    const altitude = await screen.findByRole('spinbutton', { name: 'Minimum altitude' });
    await waitFor(() => expect(altitude).toHaveValue(35));
    store({ ...stored(), revision: 1, scheduling: { minimum_altitude_degrees: 40 } });
    await act(() => client.refetchQueries({ queryKey: ['observingSettings', 'project', project] }));
    await waitFor(() => expect(altitude).toHaveValue(40));
    expect(drafts.current!.unsaved).toEqual([]);
    expect(screen.queryByText(/changed elsewhere/)).not.toBeInTheDocument();

    fireEvent.change(screen.getByRole('spinbutton', { name: 'Minimum time' }), { target: { value: '60' } });
    store({ ...stored(), revision: 2, scheduling: { minimum_altitude_degrees: 45 } });
    await act(() => client.refetchQueries({ queryKey: ['observingSettings', 'project', project] }));
    expect(await screen.findByText(/These limits changed elsewhere/)).toBeInTheDocument();
    expect(screen.getByRole('spinbutton', { name: 'Minimum time' })).toHaveValue(60);
    expect(altitude).toHaveValue(40);
    // Discard drops the edits and shows the newer limits.
    act(() => drafts.current!.discardAll());
    await waitFor(() => expect(altitude).toHaveValue(45));
    expect(screen.getByRole('spinbutton', { name: 'Minimum time' })).toHaveValue(null);
    expect(screen.queryByText(/changed elsewhere/)).not.toBeInTheDocument();
  });

  it('is read only without write access', async () => {
    fixture();
    mount(false);
    expect(await screen.findByRole('spinbutton', { name: 'Minimum altitude' })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'Custom horizon' })).toBeDisabled();
  });
});
