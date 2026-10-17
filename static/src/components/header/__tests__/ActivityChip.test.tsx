import { describe, expect, it } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import type { StackActivityEntry } from '../../../api/types';
import ActivityChip from '../ActivityChip';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function scanProgress(running: boolean, processed: number, errors = 0) {
  return {
    started: false,
    cached_count: 0,
    progress: {
      running, stage: 'astrometry', target_id: 42, filter_name: null, total: 10, processed,
      skipped_cached: 0, spatial_processed: 10, astrometry_processed: processed, solved: processed,
      solve_failed: errors, operational_errors: 0, errors, current_file: 'sh2-86-r-004.fits',
      started_at: 1, finished_at: running ? null : 2,
      last_error: errors > 0 ? 'no stars found' : null,
    },
  };
}

function stack(
  jobId: string,
  state: 'running' | 'queued',
  processed = 0,
  total = 0,
  queuePosition: number | null = null
): StackActivityEntry {
  return {
    kind: 'mono', job_id: jobId, database_id: 'askar', project_id: 1, state,
    label: `Alpha M44 · ${jobId}`, detail: 'registering', processed_units: processed,
    total_units: total, created_unix_seconds: 1, queue_position: queuePosition,
  };
}

function mockServer({
  scan = () => scanProgress(false, 0),
  stacks = () => [] as StackActivityEntry[],
  scheduled = () => [] as unknown[],
  reapplies = () => [] as unknown[],
}: {
  scan?: () => unknown;
  stacks?: () => StackActivityEntry[];
  scheduled?: () => unknown[];
  reapplies?: () => unknown[];
}) {
  server.use(
    http.get('/api/databases', () => ok([{ id: 'askar', name: 'Askar', database_path: '/x.sqlite' }])),
    http.get('/api/db/:dbId/targets', () =>
      ok([{ id: 42, project_id: 1, name: 'Alpha M44', active: true, has_files: true }])
    ),
    http.get('/api/db/:dbId/analysis/quality-scan', () => ok(scan())),
    http.get('/api/stack-activity', () =>
      ok({ schema_version: 1, active: stacks(), scheduled: scheduled(), waiting_reapplies: reapplies() })
    )
  );
}

/** The chip's or list's icon counts, by the words each stands for. */
function counts(element: HTMLElement): string[] {
  return Array.from(element.querySelectorAll('.activity-count')).map(
    (count) => count.getAttribute('title') ?? ''
  );
}

function Location() {
  const location = useLocation();
  return <output aria-label="Location">{`${location.pathname}${location.search}`}</output>;
}

function renderChip({ showLocation = false } = {}) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={['/overview']}>
        {children}
        {showLocation && (
          <Routes>
            <Route path="*" element={<Location />} />
          </Routes>
        )}
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<ActivityChip />, { wrapper: Wrapper });
}

describe('ActivityChip', () => {
  it('stays out of the header when nothing runs', async () => {
    mockServer({});
    const view = renderChip();
    // Give every status query a chance to answer.
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    // Only the empty live region stays, for announcements.
    expect(view.container.querySelector('.activity-chip')).toBeNull();
    expect(view.container).toHaveTextContent('');
  });

  it('shows the overall progress and job count, and the queue on hover', async () => {
    mockServer({
      scan: () => scanProgress(true, 4),
      stacks: () => [stack('B', 'queued'), stack('R', 'running', 8, 10)],
    });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    // A build a person queued is queued, not waiting, and counted as such.
    await waitFor(() => expect(counts(chip)).toEqual(['2 running', '1 queued']));
    expect(chip.querySelector('.activity-count[data-job-state="queued"] svg')).toHaveAttribute('aria-hidden', 'true');
    // The scan is 40% and the running build 80%; the queued one waits.
    expect(chip).toHaveTextContent('60%');
    expect(chip).toHaveAccessibleName('Background jobs: 60% done, 2 running, 1 queued');
    expect(chip).toHaveAttribute('aria-expanded', 'false');

    await userEvent.hover(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(chip).toHaveAttribute('aria-expanded', 'true');
    const rows = within(list).getAllByRole('listitem');
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining('Analyzing quality'),
      expect.stringContaining('Alpha M44 · R'),
      expect.stringContaining('Alpha M44 · B'),
    ]);
    expect(rows[0]).toHaveTextContent('Solving 4/10 frames');
    expect(rows[0]).toHaveTextContent('Askar');
    expect(rows[1]).toHaveTextContent('8/10 frames');
    expect(rows[2]).toHaveTextContent('queued');
    // All of it a person started: one group, named by its heading, with its
    // counts. The list's heading comes first, then the group's, a level down.
    const groups = within(list).getAllByRole('group');
    expect(groups.map((group) => within(group).getByRole('heading', { level: 3 }).textContent))
      .toEqual(['Started by you']);
    expect(groups[0]).toHaveAccessibleName('Started by you');
    expect(within(list).getByRole('heading', { level: 2 })).toHaveTextContent('Background jobs');
    expect(counts(groups[0].querySelector('.activity-group-head')!)).toEqual(['2 running', '1 queued']);

    await userEvent.unhover(chip);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull());
  });

  it('lists automatic work waiting for a person\'s build, then shows it running once, after', async () => {
    let personBuilding = true;
    const refresh = {
      database_id: 'askar', database_name: 'Askar', project_id: 1, project_name: 'Alpha',
      reason: 'arrival', due_in_seconds: 0, channels: ['Alpha M44 · Ha'], waiting: true,
      waiting_for: 'the stack build of Alpha M44 · R', last_progress: {
        label: 'Alpha M44 · Ha', processed_units: 12, total_units: 40,
      },
    };
    const settling = { ...refresh, project_id: 2, waiting: false, waiting_for: null, last_progress: null };
    mockServer({
      scan: () => ({
        ...scanProgress(true, 4),
        progress: {
          ...scanProgress(true, 4).progress,
          automatic: true,
          waiting_for: personBuilding ? 'a stack build' : null,
        },
      }),
      stacks: () => (personBuilding
        ? [stack('R', 'running', 3, 10)]
        : [{ ...stack('Ha', 'running', 13, 40), automatic: true }]),
      scheduled: () => (personBuilding ? [refresh, settling] : [settling]),
      reapplies: () => (personBuilding
        ? [{ database_id: 'askar', project_id: 1, target_id: 42, label: 'Alpha M44 · G', waiting_for: 'a stack build' }]
        : []),
    });
    renderChip();
    const chip = () => screen.getByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip())).toEqual(['1 running', '3 waiting']));
    // Icons with counts; the words are there for a screen reader.
    expect(within(chip()).getByText('3 waiting')).toHaveClass('activity-count-name');
    expect(within(chip()).getByTitle('3 waiting').querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
    // Only the person's build counts toward the progress.
    expect(chip()).toHaveAccessibleName('Background jobs: 30% done, 1 running, 3 waiting');
    await userEvent.click(chip());
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(counts(list.querySelector('.activity-popover-head')!)).toEqual(['1 running', '3 waiting']);
    let rows = within(list).getAllByRole('listitem');
    // The person's build first; the settling refresh is not work yet.
    expect(rows.map((row) => row.getAttribute('data-job-state'))).toEqual([
      'running', 'waiting', 'waiting', 'waiting',
    ]);
    expect(rows[0]).toHaveTextContent('Alpha M44 · R');
    expect(rows[1]).toHaveTextContent('Analyzing quality');
    expect(rows[1]).toHaveTextContent('Waiting for a stack build · Solving 4/10 frames');
    expect(rows[2]).toHaveTextContent('Refreshing stacks');
    expect(rows[2]).toHaveTextContent('Askar · Alpha');
    expect(rows[2]).toHaveTextContent(
      'Waiting for the stack build of Alpha M44 · R · stopped at 12/40 frames'
    );
    // Under the automatic heading, apart from the person's build.
    const person = within(list).getByRole('group', { name: 'Started by you' });
    const automatic = within(list).getByRole('group', { name: 'Automatic' });
    expect(within(person).getAllByRole('listitem')).toEqual([rows[0]]);
    expect(within(automatic).getAllByRole('listitem')).toEqual(rows.slice(1));
    expect(counts(automatic.querySelector('.activity-group-head')!)).toEqual(['3 waiting']);
    expect(within(rows[2]).getByText('waiting')).toHaveClass('activity-row-state');
    // It cannot be stopped from here, and has no place in a line.
    expect(within(rows[2]).queryByRole('button')).toBeNull();
    expect(rows[3]).toHaveTextContent('Re-applying processing');
    expect(rows[3]).toHaveTextContent('Waiting for a stack build');

    // The person's build ends: the refresh runs as its build, listed once.
    personBuilding = false;
    await waitFor(() => expect(counts(chip())).toEqual(['2 running']), { timeout: 3000 });
    rows = within(screen.getByRole('region', { name: 'Background jobs' })).getAllByRole('listitem');
    expect(rows).toHaveLength(2);
    expect(rows.some((row) => row.textContent?.includes('Refreshing stacks'))).toBe(false);
    expect(rows.find((row) => row.textContent?.includes('Alpha M44 · Ha'))).toHaveAttribute(
      'data-job-state',
      'running'
    );
  }, 10_000);

  it('shows no running count when only waiting work is left', async () => {
    mockServer({
      reapplies: () => [
        { database_id: 'askar', project_id: 1, target_id: 42, label: 'Alpha M44 · G', waiting_for: 'a preview' },
      ],
    });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip)).toEqual(['1 waiting']));
    expect(chip).toHaveAccessibleName('Background jobs: 1 waiting');
  });

  it('keeps the queue open after a click until a second click or Escape', async () => {
    mockServer({ stacks: () => [stack('R', 'running', 1, 4)] });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await userEvent.click(chip);
    await userEvent.unhover(chip);
    await act(() => new Promise((resolve) => setTimeout(resolve, 300)));
    expect(screen.getByRole('region', { name: 'Background jobs' })).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull();

    await userEvent.click(chip);
    expect(screen.getByRole('region', { name: 'Background jobs' })).toBeInTheDocument();
    await userEvent.click(chip);
    await userEvent.unhover(chip);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull());
  });

  it('keeps saying a scan finished with errors until they are dismissed', async () => {
    let running = true;
    mockServer({ scan: () => (running ? scanProgress(true, 9) : scanProgress(false, 10, 2)) });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    expect(chip).toHaveTextContent('90%');
    running = false;
    const done = await screen.findByText('Finished with errors', {}, { timeout: 3000 });
    expect(done.closest('.activity-chip')).toHaveAttribute(
      'title',
      expect.stringContaining('Askar: 2 frames failed — no stars found')
    );
    // Well past the moment "Done" would have gone.
    await act(() => new Promise((resolve) => setTimeout(resolve, 3000)));
    const finished = screen.getByRole('button', { name: 'Background jobs: finished with errors' });

    await userEvent.click(finished);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(within(list).getByRole('note')).toHaveTextContent('Askar: 2 frames failed — no stars found');
    expect(within(list).getByRole('link', { name: 'Open the sequence' })).toHaveAttribute(
      'href',
      '/sequence?db=askar&project=1&target=42'
    );
    await userEvent.click(within(list).getByRole('button', { name: 'Dismiss the quality scan errors in Askar' }));
    await waitFor(() => expect(screen.queryByText('Finished with errors')).toBeNull());
    expect(screen.queryByRole('button', { name: /Background jobs/ })).toBeNull();
  }, 10_000);

  it('dismisses scan errors from the busy chip while other work runs', async () => {
    let scanning = true;
    mockServer({
      scan: () => (scanning ? scanProgress(true, 9) : scanProgress(false, 10, 2)),
      stacks: () => [stack('R', 'running', 1, 3)],
    });
    renderChip();
    // Looked up each time: the chip may be drawn anew as a poll lands.
    const chip = () => screen.getByRole('button', { name: /Background jobs/ });
    // The scan must be seen running before it ends: frame errors belong only
    // to a scan the header watched. The build alone shows the chip first.
    await waitFor(() => expect(chip()).toHaveAccessibleName(/2 running/));
    scanning = false;
    await waitFor(() => expect(chip()).toHaveAccessibleName(/a quality scan had errors/), { timeout: 8000 });
    await userEvent.click(chip());
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    await userEvent.click(within(list).getByRole('button', { name: 'Dismiss the quality scan errors in Askar' }));
    await waitFor(() => expect(chip()).not.toHaveAccessibleName(/had errors/));
    expect(within(list).queryByRole('note')).toBeNull();
    expect(counts(chip())).toEqual(['1 running']);
  }, 15_000);

  it('takes a row to the view showing what it works on', async () => {
    mockServer({ stacks: () => [{ ...stack('R', 'running', 1, 3), target_id: 7 }] });
    renderChip({ showLocation: true });
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await userEvent.click(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    await userEvent.click(within(list).getByRole('link', { name: 'Go to Stacking: Alpha M44 · R' }));
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent(
      '/stacks?db=askar&project=1&target=7'
    );
    // The list closes as the view changes.
    expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull();
  });

  it("does not report an old scan's errors when other work finishes", async () => {
    let building = true;
    // The server keeps the last scan's counts until the next scan starts.
    mockServer({
      scan: () => scanProgress(false, 10, 3),
      stacks: () => (building ? [stack('R', 'running', 1, 3)] : []),
    });
    renderChip();
    await screen.findByRole('button', { name: /Background jobs/ });
    building = false;
    expect(await screen.findByText('Done', {}, { timeout: 7000 })).toBeInTheDocument();
    expect(screen.queryByText('Finished with errors')).toBeNull();
  }, 10_000);

  it('holds scan errors on the busy chip until the rest of the queue ends', async () => {
    let scanning = true;
    let building = true;
    mockServer({
      scan: () => (scanning ? scanProgress(true, 9) : scanProgress(false, 10, 2)),
      stacks: () => (building ? [stack('R', 'running', 1, 3)] : []),
    });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip)).toEqual(['2 running']));
    scanning = false;
    await waitFor(() => expect(chip).toHaveAccessibleName(/a quality scan had errors/), { timeout: 3000 });
    expect(counts(chip)).toEqual(['1 running']);
    await userEvent.hover(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(within(list).getByRole('note')).toHaveTextContent('Askar: 2 frames failed — no stars found');
    // A screen reader hears it too.
    expect(document.querySelector('.activity-live')).toHaveTextContent('A quality scan finished with errors');

    await userEvent.unhover(chip);
    building = false;
    expect(await screen.findByText('Finished with errors', {}, { timeout: 7000 })).toBeInTheDocument();
  }, 12_000);

  it('leaves Escape to the view unless the chip has focus', async () => {
    mockServer({ stacks: () => [stack('R', 'running', 1, 4)] });
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    const heard: string[] = [];
    const listen = (event: KeyboardEvent) => heard.push(event.key);
    document.addEventListener('keydown', listen);
    try {
      await userEvent.hover(chip);
      await screen.findByRole('region', { name: 'Background jobs' });
      (document.activeElement as HTMLElement | null)?.blur();
      await userEvent.keyboard('{Escape}');
      expect(heard).toEqual(['Escape']);
      expect(screen.getByRole('region', { name: 'Background jobs' })).toBeInTheDocument();

      chip.focus();
      await userEvent.keyboard('{Escape}');
      expect(heard).toEqual(['Escape']);
      expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull();
    } finally {
      document.removeEventListener('keydown', listen);
    }
  });

  it('moves a waiting build, removes one, and stops a running one after asking', async () => {
    const calls: string[] = [];
    let active = [
      stack('R', 'running', 1, 4),
      stack('B', 'queued', 0, 0, 0),
      stack('G', 'queued', 0, 0, 1),
    ];
    mockServer({ stacks: () => active });
    server.use(
      http.post('/api/stack-activity/:jobId/move', async ({ params, request }) => {
        calls.push(`move ${params.jobId} ${JSON.stringify(await request.json())}`);
        active = [stack('R', 'running', 1, 4), stack('G', 'queued', 0, 0, 0), stack('B', 'queued', 0, 0, 1)];
        return ok({ schema_version: 1, active });
      }),
      http.post('/api/stack-activity/:jobId/cancel', ({ params }) => {
        calls.push(`cancel ${params.jobId}`);
        active = active.filter((entry) => entry.job_id !== params.jobId);
        return ok({ schema_version: 1, active });
      })
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });

    // The first in line cannot go earlier, nor the last later.
    expect(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · B earlier' })).toBeDisabled();
    expect(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · G later' })).toBeDisabled();
    await userEvent.click(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · G earlier' }));
    await waitFor(() => expect(calls).toEqual(['move G {"position":0}']));

    await waitFor(() => expect(within(list).getByRole('button', { name: 'Remove Stacking: Alpha M44 · B from the line' })).toBeEnabled());
    await userEvent.click(within(list).getByRole('button', { name: 'Remove Stacking: Alpha M44 · B from the line' }));
    await waitFor(() => expect(calls).toContain('cancel B'));

    await userEvent.click(within(list).getByRole('button', { name: 'Stop Stacking: Alpha M44 · R' }));
    expect(calls).not.toContain('cancel R');
    await userEvent.click(within(list).getByRole('button', { name: 'Stop it' }));
    await waitFor(() => expect(calls).toContain('cancel R'));
  });

  it("moves a person's build past an automatic one queued ahead of it, across the headings", async () => {
    const calls: string[] = [];
    mockServer({
      stacks: () => [
        stack('R', 'running', 1, 4),
        { ...stack('Ha', 'queued', 0, 0, 0), automatic: true },
        stack('B', 'queued', 0, 0, 1),
      ],
    });
    server.use(
      http.post('/api/stack-activity/:jobId/move', async ({ params, request }) => {
        calls.push(`move ${params.jobId} ${JSON.stringify(await request.json())}`);
        return ok({ schema_version: 1, active: [] });
      })
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    const person = within(list).getByRole('group', { name: 'Started by you' });
    const automatic = within(list).getByRole('group', { name: 'Automatic' });
    // First of its heading's queued rows, yet one build runs before it.
    const mine = within(person).getAllByRole('listitem')[1];
    expect(mine).toHaveTextContent('Alpha M44 · B');
    expect(mine).toHaveTextContent('queued · 2 · 1 ahead');
    expect(within(automatic).getByRole('listitem')).toHaveTextContent('queued · 1 · next');
    // Up goes past the automatic build; it is last in the line, so down is off.
    expect(within(mine).getByRole('button', { name: 'Run Stacking: Alpha M44 · B later' })).toBeDisabled();
    await userEvent.click(within(mine).getByRole('button', { name: 'Run Stacking: Alpha M44 · B earlier' }));
    await waitFor(() => expect(calls).toEqual(['move B {"position":0}']));
  });

  it('counts a transfer held for review apart from waiting work, first under its heading', async () => {
    mockServer({ stacks: () => [stack('R', 'running', 1, 4)] });
    const transfer = (id: string, state: string, automatic: boolean) => ({
      id, connection_id: 'c', revision: 1, state, catalog: 'Askar', label: `M44 ${id}`, total_files: 4,
      completed_files: 1, total_bytes: 4, completed_bytes: 1, current_file: '', sent_bytes: 0,
      error: state === 'review' ? 'Incomplete calibration' : null, next_attempt_ms: 0, warnings: null,
      manifest: null, automatic,
    });
    server.use(http.get('/api/director/v1/transfers', () => ok({
      jobs: [transfer('paused', 'paused', true), transfer('held', 'review', true)],
    })));
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip)).toEqual(['1 running', '1 waiting', '1 needs attention']));
    expect(chip).toHaveAccessibleName('Background jobs: 25% done, 1 running, 1 waiting, 1 needs attention');
    await userEvent.click(chip);
    const automatic = within(await screen.findByRole('region', { name: 'Background jobs' }))
      .getByRole('group', { name: 'Automatic' });
    expect(counts(automatic.querySelector('.activity-group-head')!)).toEqual(['1 waiting', '1 needs attention']);
    const rows = within(automatic).getAllByRole('listitem');
    expect(rows.map((row) => row.getAttribute('data-job-state'))).toEqual(['attention', 'waiting']);
    expect(rows[0]).toHaveTextContent('Review required');
  });

  it('shows WBPP runs, and gives their controls to an editor without database management', async () => {
    mockServer({});
    server.use(
      http.get('/api/info', () => ok({ version: 'test', allow_database_management: false })),
      http.get('/api/wbpp/activity', () => ok({
        running: [{
          db_id: 'askar', db_name: 'Askar', scope: 'project Bubble', stage: 'running',
          wbpp_stage: 'Calibration', wbpp_steps: 2, started_at: 1,
        }],
        queued: [],
      }))
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Askar · project Bubble');
    expect(list).toHaveTextContent('Running in PixInsight · Calibration · 2 steps done');
    // A run uses the folders the PixInsight settings define: an editor's.
    expect(await within(list).findByRole('button', { name: 'Stop WBPP: Askar · project Bubble' })).toBeInTheDocument();
  });

  it('neither lists nor counts an automatic refresh still settling', async () => {
    let building = false;
    mockServer({
      scheduled: () => [{
        database_id: 'askar', database_name: 'Askar', project_id: 7, project_name: 'Heart',
        reason: 'arrival', due_in_seconds: 240,
      }],
      stacks: () => (building ? [stack('R', 'running', 1, 3)] : []),
    });
    const view = renderChip();
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    // Nothing runs yet: no chip at all.
    expect(view.container.querySelector('.activity-chip')).toBeNull();

    // Once it starts, it is an automatic build like any other.
    building = true;
    const chip = await screen.findByRole('button', { name: /Background jobs/ }, { timeout: 7000 });
    expect(counts(chip)).toEqual(['1 running']);
    await userEvent.click(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).not.toHaveTextContent('Automatic refresh');
  }, 10_000);

  it('lists a processing run beside the build it waits for, then its progress, and links to its stacks', async () => {
    let running = false;
    const blur = (): StackActivityEntry => ({
      kind: 'processing', job_id: 'b'.repeat(64), database_id: 'askar', project_id: 1, target_id: 42,
      state: running ? 'running' : 'queued', label: 'Alpha M44 · R',
      detail: running ? 'BlurXTerminator' : 'Waiting for the stack build of Alpha M44 · Ha to finish',
      processed_units: running ? 420 : 0, total_units: running ? 1000 : 0, created_unix_seconds: 2,
      queue_position: running ? null : 0,
      progress_label: running ? 'BlurXTerminator · 42%' : null,
      processing: {
        operation: 'BlurXTerminator', channel: 'R', stoppable: true,
        stage: running ? 'BlurXTerminator' : null, fraction: running ? 0.42 : null,
        waiting_for: running ? null : 'the stack build of Alpha M44 · Ha',
      },
    });
    mockServer({
      stacks: () => (running ? [blur()] : [{ ...stack('Ha', 'running', 3, 10), target_id: 42 }, blur()]),
    });
    renderChip({ showLocation: true });
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(chip).toHaveAccessibleName('Background jobs: 30% done, 1 running, 1 queued'));
    await userEvent.click(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    const rows = within(list).getAllByRole('listitem');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('Stacking');
    expect(rows[1]).toHaveTextContent('BlurXTerminator');
    expect(rows[1]).toHaveTextContent('queued · 1 · next');
    expect(rows[1]).toHaveTextContent('Alpha M44 · R');
    expect(rows[1]).toHaveTextContent('Waiting for the stack build of Alpha M44 · Ha');
    // It waits in the builds' line, so it leaves it the same way.
    expect(within(rows[1]).getByRole('button', { name: 'Remove BlurXTerminator: Alpha M44 · R from the line' }))
      .toBeInTheDocument();

    running = true;
    await waitFor(() => expect(within(list).getAllByRole('listitem')).toHaveLength(1), { timeout: 3000 });
    const row = within(list).getByRole('listitem');
    expect(row).toHaveTextContent('42%');
    expect(row).toHaveTextContent('BlurXTerminator');
    expect(within(row).getByRole('button', { name: 'Stop BlurXTerminator: Alpha M44 · R' })).toBeInTheDocument();
    await userEvent.click(within(row).getByRole('link', { name: 'Go to BlurXTerminator: Alpha M44 · R' }));
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent(
      '/stacks?db=askar&project=1&target=42'
    );
  }, 10_000);

  it('offers no Stop on a running color composition', async () => {
    mockServer({ stacks: () => [{ ...stack('C', 'running', 1, 4), kind: 'color' }] });
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Composing color');
    expect(within(list).queryByRole('button', { name: /^Stop / })).toBeNull();
  });
});
