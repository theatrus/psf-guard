import { describe, expect, it } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
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
}: {
  scan?: () => unknown;
  stacks?: () => StackActivityEntry[];
  scheduled?: () => unknown[];
}) {
  server.use(
    http.get('/api/databases', () => ok([{ id: 'askar', name: 'Askar', database_path: '/x.sqlite' }])),
    http.get('/api/db/:dbId/analysis/quality-scan', () => ok(scan())),
    http.get('/api/stack-activity', () => ok({ schema_version: 1, active: stacks(), scheduled: scheduled() }))
  );
}

function renderChip() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
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
    await waitFor(() => expect(chip).toHaveTextContent('3 jobs'));
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
    expect(rows[2]).toHaveTextContent('queued');

    await userEvent.unhover(chip);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull());
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

  it('says a scan finished with errors, then leaves', async () => {
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
    await waitFor(() => expect(screen.queryByText('Finished with errors')).toBeNull(), { timeout: 4000 });
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
    await waitFor(() => expect(chip).toHaveTextContent('2 jobs'));
    scanning = false;
    await waitFor(() => expect(chip).toHaveAccessibleName(/a quality scan had errors/), { timeout: 3000 });
    expect(chip).toHaveTextContent('1 job');
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
    mockServer({
      stacks: () => [
        stack('R', 'running', 1, 4),
        stack('B', 'queued', 0, 0, 0),
        stack('G', 'queued', 0, 0, 1),
      ],
    });
    server.use(
      http.post('/api/stack-activity/:jobId/move', async ({ params, request }) => {
        calls.push(`move ${params.jobId} ${JSON.stringify(await request.json())}`);
        return ok({ schema_version: 1, active: [] });
      }),
      http.post('/api/stack-activity/:jobId/cancel', ({ params }) => {
        calls.push(`cancel ${params.jobId}`);
        return ok({ schema_version: 1, active: [] });
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

    await userEvent.click(within(list).getByRole('button', { name: 'Remove Stacking: Alpha M44 · B from the line' }));
    await waitFor(() => expect(calls).toContain('cancel B'));

    await userEvent.click(within(list).getByRole('button', { name: 'Stop Stacking: Alpha M44 · R' }));
    expect(calls).not.toContain('cancel R');
    await userEvent.click(within(list).getByRole('button', { name: 'Stop it' }));
    await waitFor(() => expect(calls).toContain('cancel R'));
  });

  it('shows WBPP runs, and leaves their controls to a server that manages databases', async () => {
    mockServer({});
    let management = false;
    server.use(
      http.get('/api/info', () => ok({ version: 'test', allow_database_management: management })),
      http.get('/api/wbpp/activity', () => ok({
        running: [{
          db_id: 'askar', db_name: 'Askar', scope: 'project Bubble', stage: 'running',
          wbpp_stage: 'Calibration', wbpp_steps: 2, started_at: 1,
        }],
        queued: [],
      }))
    );
    const view = renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Askar · project Bubble');
    expect(list).toHaveTextContent('Running in PixInsight · Calibration · 2 steps done');
    expect(within(list).queryByRole('button', { name: /Stop WBPP/ })).toBeNull();

    view.unmount();
    management = true;
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    expect(await screen.findByRole('button', { name: 'Stop WBPP: Askar · project Bubble' })).toBeInTheDocument();
  });

  it('lists a settling automatic refresh to run now or skip, without a spinning ring', async () => {
    const calls: string[] = [];
    mockServer({
      scheduled: () => [{
        database_id: 'askar', database_name: 'Askar', project_id: 7, project_name: 'Heart',
        reason: 'arrival', due_in_seconds: 240,
      }],
    });
    server.use(
      http.post('/api/stack-activity/scheduled/:action', async ({ params, request }) => {
        calls.push(`${params.action} ${JSON.stringify(await request.json())}`);
        return ok({ schema_version: 1, active: [], scheduled: [] });
      })
    );
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    expect(chip).toHaveTextContent('1 job');
    expect(chip.querySelector('.activity-ring.is-indeterminate')).toBeNull();
    await userEvent.click(chip);
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Automatic refresh');
    expect(list).toHaveTextContent('After new frames · starts in 4 min');
    await userEvent.click(within(list).getByRole('button', { name: 'Run Automatic refresh: Askar · Heart now' }));
    await waitFor(() => expect(calls).toEqual(['run-now {"database_id":"askar","project_id":7}']));
  });

  it('offers no Stop on a running color composition', async () => {
    mockServer({ stacks: () => [{ ...stack('C', 'running', 1, 4), kind: 'color' }] });
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Composing color');
    expect(within(list).queryByRole('button', { name: /^Stop / })).toBeNull();
  });
});
