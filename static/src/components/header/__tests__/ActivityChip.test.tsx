import { describe, expect, it } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import type { DatabaseJobs, JobRow } from '../../../api/types';
import { AccessContext } from '../../../auth/access';
import ActivityChip from '../ActivityChip';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

/** A row as `GET /api/jobs` returns it. */
function row(overrides: Partial<JobRow> & Pick<JobRow, 'key' | 'title' | 'scope'>): JobRow {
  return {
    kind: 'stack', detail: '', status: 'running', background: false, automatic: false,
    percent: null, stoppable: false, ...overrides,
  };
}

/** A mono build's row: running with its frames, or queued at `position`. */
function stackRow(
  jobId: string,
  state: 'running' | 'queued',
  processed = 0,
  total = 0,
  position: number | null = null,
  automatic = false
): JobRow {
  const queued = state === 'queued';
  return row({
    key: `stack:${jobId}`,
    kind: 'stack',
    title: 'Stacking',
    scope: `Alpha M44 · ${jobId}`,
    detail: queued ? 'Waiting for the build ahead' : total > 0 ? `${processed}/${total} frames` : 'registering',
    status: queued ? 'queued' : 'running',
    percent: queued || total === 0 ? null : (processed / total) * 100,
    queue: 'stack',
    position: queued ? position ?? undefined : undefined,
    control: { kind: 'stack', job_id: jobId },
    stoppable: true,
    automatic,
    background: automatic,
    hint: 'registering',
    link: { view: 'stacks', db: 'askar', project: 1 },
  });
}

/** A quality scan's row on Askar. */
function scanRow(processed: number, waitingFor?: string): JobRow {
  const frames = `Solving ${processed}/10 frames`;
  return row({
    key: 'scan:askar',
    kind: 'quality',
    title: 'Analyzing quality',
    scope: 'Askar',
    detail: waitingFor ? `Waiting for ${waitingFor} · ${frames}` : frames,
    status: waitingFor ? 'waiting' : 'running',
    waiting_for: waitingFor,
    percent: processed * 10,
    automatic: !!waitingFor,
    background: !!waitingFor,
    link: { view: 'sequence', db: 'askar', project: 1, target: 42 },
  });
}

/** Askar's state: whether a scan runs, and the last scan's frame errors. */
function askar(scanning = false, errors = 0): DatabaseJobs {
  return {
    id: 'askar', name: 'Askar', busy: scanning, scanning,
    scan: { errors, last_error: errors > 0 ? 'no stars found' : null, target_id: 42, project_id: 1 },
  };
}

function mockJobs(jobs: () => JobRow[], databases: () => DatabaseJobs[] = () => [askar()]) {
  server.use(http.get('/api/jobs', () => ok({ jobs: jobs(), databases: databases() })));
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

function renderChip({ showLocation = false, canWrite = true } = {}) {
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
  return render(
    <AccessContext.Provider value={{ canWrite, canCompute: canWrite,
      status: { authentication_required: true, authenticated: true, role: canWrite ? 'read_write' : 'read_only', can_compute: canWrite },
      logout: async () => undefined }}>
      <ActivityChip />
    </AccessContext.Provider>,
    { wrapper: Wrapper }
  );
}

describe('ActivityChip', () => {
  it('stays out of the header when nothing runs', async () => {
    mockJobs(() => []);
    const view = renderChip();
    // Give the job list a chance to answer.
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    // Only the empty live region stays, for announcements.
    expect(view.container.querySelector('.activity-chip')).toBeNull();
    expect(view.container).toHaveTextContent('');
  });

  it('shows the overall progress and job count, and the queue on hover', async () => {
    mockJobs(() => [scanRow(4), stackRow('R', 'running', 8, 10), stackRow('B', 'queued', 0, 0, 0)]);
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
    expect(rows.map((item) => item.textContent)).toEqual([
      expect.stringContaining('Analyzing quality'),
      expect.stringContaining('Alpha M44 · R'),
      expect.stringContaining('Alpha M44 · B'),
    ]);
    expect(rows[0]).toHaveTextContent('Solving 4/10 frames');
    expect(rows[0]).toHaveTextContent('Askar');
    expect(rows[1]).toHaveTextContent('8/10 frames');
    expect(rows[2]).toHaveTextContent('queued');
    // All of it a person started: one group, with its heading and counts.
    // The list's heading comes first, then the group's, a level down.
    const groups = within(list).getAllByRole('group');
    expect(groups.map((group) => within(group).getByRole('heading', { level: 3 }).textContent))
      .toEqual(['Started by you']);
    expect(groups[0]).toHaveAccessibleName('Started by you');
    expect(within(list).getByRole('heading', { level: 2 })).toHaveTextContent('Background jobs');
    expect(counts(groups[0].querySelector('.activity-group-head')!)).toEqual(['2 running', '1 queued']);

    await userEvent.unhover(chip);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Background jobs' })).toBeNull());
  });

  it("lists automatic work waiting for a person's build, then shows it running once, after", async () => {
    let personBuilding = true;
    const refreshWaiting = row({
      key: 'refresh-waiting:askar:1', kind: 'stack', title: 'Refreshing stacks', scope: 'Askar · Alpha',
      detail: 'Waiting for the stack build of Alpha M44 · R · stopped at 12/40 frames', status: 'waiting',
      waiting_for: 'the stack build of Alpha M44 · R', percent: 30, automatic: true, background: true,
      remaining: '1 channel to stack',
    });
    const reapply = row({
      key: 'reapply-waiting:askar:1:Alpha M44 · G', kind: 'processing', title: 'Re-applying processing',
      scope: 'Alpha M44 · G', detail: 'Waiting for a stack build', status: 'waiting',
      waiting_for: 'a stack build', automatic: true, background: true,
    });
    mockJobs(() => (personBuilding
      ? [stackRow('R', 'running', 3, 10), scanRow(4, 'a stack build'), refreshWaiting, reapply]
      : [stackRow('Ha', 'running', 13, 40, null, true), { ...scanRow(4), automatic: true, background: true }]));
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
    expect(rows.map((item) => item.getAttribute('data-job-state'))).toEqual([
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
    expect(rows[2]).toHaveTextContent('1 channel to stack');
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
    expect(rows.some((item) => item.textContent?.includes('Refreshing stacks'))).toBe(false);
    expect(rows.find((item) => item.textContent?.includes('Alpha M44 · Ha'))).toHaveAttribute(
      'data-job-state',
      'running'
    );
  }, 10_000);

  it('shows no running count when only waiting work is left', async () => {
    mockJobs(() => [row({
      key: 'reapply-waiting:askar:1:G', kind: 'processing', title: 'Re-applying processing',
      scope: 'Alpha M44 · G', detail: 'Waiting for a preview', status: 'waiting',
      waiting_for: 'a preview', automatic: true, background: true,
    })]);
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip)).toEqual(['1 waiting']));
    expect(chip).toHaveAccessibleName('Background jobs: 1 waiting');
  });

  it('keeps the queue open after a click until a second click or Escape', async () => {
    mockJobs(() => [stackRow('R', 'running', 1, 4)]);
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
    mockJobs(
      () => (running ? [scanRow(9)] : []),
      () => [running ? askar(true) : askar(false, 2)]
    );
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
    mockJobs(
      () => (scanning ? [scanRow(9), stackRow('R', 'running', 1, 3)] : [stackRow('R', 'running', 1, 3)]),
      () => [scanning ? askar(true) : askar(false, 2)]
    );
    renderChip();
    // Looked up each time: the chip may be drawn anew as a poll lands.
    const chip = () => screen.getByRole('button', { name: /Background jobs/ });
    // The scan must be seen running before it ends: frame errors belong only
    // to a scan the header watched.
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
    mockJobs(() => [{ ...stackRow('R', 'running', 1, 3), link: { view: 'stacks', db: 'askar', project: 1, target: 7 } }]);
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
    mockJobs(
      () => (building ? [stackRow('R', 'running', 1, 3)] : []),
      () => [askar(false, 3)]
    );
    renderChip();
    await screen.findByRole('button', { name: /Background jobs/ });
    building = false;
    expect(await screen.findByText('Done', {}, { timeout: 7000 })).toBeInTheDocument();
    expect(screen.queryByText('Finished with errors')).toBeNull();
  }, 10_000);

  it('holds scan errors on the busy chip until the rest of the queue ends', async () => {
    let scanning = true;
    let building = true;
    mockJobs(
      () => [...(scanning ? [scanRow(9)] : []), ...(building ? [stackRow('R', 'running', 1, 3)] : [])],
      () => [scanning ? askar(true) : askar(false, 2)]
    );
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
    mockJobs(() => [stackRow('R', 'running', 1, 4)]);
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
    let rows = [
      stackRow('R', 'running', 1, 4),
      stackRow('B', 'queued', 0, 0, 0),
      stackRow('G', 'queued', 0, 0, 1),
    ];
    mockJobs(() => rows);
    server.use(
      http.post('/api/stack-activity/:jobId/move', async ({ params, request }) => {
        calls.push(`move ${params.jobId} ${JSON.stringify(await request.json())}`);
        rows = [stackRow('R', 'running', 1, 4), stackRow('G', 'queued', 0, 0, 0), stackRow('B', 'queued', 0, 0, 1)];
        return ok({ schema_version: 1, active: [] });
      }),
      http.post('/api/stack-activity/:jobId/cancel', ({ params }) => {
        calls.push(`cancel ${params.jobId}`);
        rows = rows.filter((item) => item.key !== `stack:${params.jobId}`);
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

    await waitFor(() => expect(within(list).getByRole('button', { name: 'Remove Stacking: Alpha M44 · B from the line' })).toBeEnabled());
    await userEvent.click(within(list).getByRole('button', { name: 'Remove Stacking: Alpha M44 · B from the line' }));
    await waitFor(() => expect(calls).toContain('cancel B'));

    await userEvent.click(within(list).getByRole('button', { name: 'Stop Stacking: Alpha M44 · R' }));
    expect(calls).not.toContain('cancel R');
    await userEvent.click(within(list).getByRole('button', { name: 'Stop it' }));
    await waitFor(() => expect(calls).toContain('cancel R'));
  });

  it('pauses preview pre-generation at once, keeps the row, and resumes it', async () => {
    const calls: string[] = [];
    const writing = row({
      key: 'pregeneration:askar', kind: 'pregeneration', title: 'Generating previews', scope: 'Askar',
      detail: '812 of 4,000 images', percent: 20.3, automatic: true, background: true,
      pause: { db_id: 'askar', job: 'pregeneration', paused: false },
    });
    let rows = [writing];
    mockJobs(() => rows);
    server.use(
      http.post('/api/db/:db/jobs/:job/pause', ({ params }) => {
        calls.push(`pause ${params.db} ${params.job}`);
        rows = [{
          ...writing, status: 'paused', detail: 'Paused · 812 of 4,000 images',
          pause: { db_id: 'askar', job: 'pregeneration', paused: true },
        }];
        return ok(true);
      }),
      http.post('/api/db/:db/jobs/:job/resume', ({ params }) => {
        calls.push(`resume ${params.db} ${params.job}`);
        rows = [writing];
        return ok(true);
      }),
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(within(list).getByText('812 of 4,000 images')).toBeInTheDocument();
    expect(within(list).queryByRole('button', { name: /^Stop/ })).toBeNull();

    // Pausing keeps the work, so it does not ask first.
    await userEvent.click(within(list).getByRole('button', { name: 'Pause Generating previews: Askar' }));
    await waitFor(() => expect(calls).toEqual(['pause askar pregeneration']));
    expect(await within(list).findByText('Paused · 812 of 4,000 images')).toBeInTheDocument();
    // The chip counts it as paused, not as waiting for other work.
    expect(screen.getByRole('button', { name: /Background jobs: 1 paused/ })).toBeInTheDocument();

    await waitFor(() => expect(within(list).getByRole('button', { name: 'Resume Generating previews: Askar' })).toBeEnabled());
    await userEvent.click(within(list).getByRole('button', { name: 'Resume Generating previews: Askar' }));
    await waitFor(() => expect(calls).toEqual(['pause askar pregeneration', 'resume askar pregeneration']));
  });

  it('keeps the move buttons disabled until the job list is read again', async () => {
    let rows = [
      stackRow('R', 'running', 1, 4),
      stackRow('B', 'queued', 0, 0, 0),
      stackRow('G', 'queued', 0, 0, 1),
    ];
    let release: (() => void) | null = null;
    let moved = false;
    server.use(
      http.get('/api/jobs', async () => {
        // The first read after the move waits until the test releases it.
        if (moved && release === null) {
          await new Promise<void>((resolve) => { release = resolve; });
        }
        return ok({ jobs: rows, databases: [askar()] });
      }),
      http.post('/api/stack-activity/:jobId/move', () => {
        moved = true;
        rows = [stackRow('R', 'running', 1, 4), stackRow('G', 'queued', 0, 0, 0), stackRow('B', 'queued', 0, 0, 1)];
        return ok({ schema_version: 1, active: [] });
      })
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    await userEvent.click(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · G earlier' }));
    await waitFor(() => expect(release).not.toBeNull());
    // The move answered, but the list still shows the old order: the
    // controls stay disabled.
    expect(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · B later' })).toBeDisabled();
    act(() => release!());
    await waitFor(() =>
      expect(within(list).getByRole('button', { name: 'Run Stacking: Alpha M44 · G later' })).toBeEnabled()
    );
  });

  it.each(['mono', 'processing'] as const)('moves a user-started %s task straight to the top across both headings', async (kind) => {
    const promoted = (position: number): JobRow => ({
      ...stackRow('G', 'queued', 0, 0, position),
      ...(kind === 'processing'
        ? { kind: 'processing' as const, title: 'BlurXTerminator', detail: 'registering' }
        : {}),
    });
    let rows = [
      stackRow('R', 'running', 1, 4),
      stackRow('B', 'queued', 0, 0, 1),
      promoted(2),
      stackRow('Ha', 'queued', 0, 0, 0, true),
    ];
    const calls: unknown[] = [];
    mockJobs(() => rows);
    server.use(http.post('/api/stack-activity/:jobId/move', async ({ params, request }) => {
      calls.push({ id: params.jobId, body: await request.json() });
      rows = [
        stackRow('R', 'running', 1, 4),
        promoted(0),
        stackRow('B', 'queued', 0, 0, 2),
        stackRow('Ha', 'queued', 0, 0, 1, true),
      ];
      return ok({ schema_version: 1, active: [] });
    }));
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    const name = `${kind === 'processing' ? 'BlurXTerminator' : 'Stacking'}: Alpha M44 · G`;
    const move = within(list).getByRole('button', { name: `Move ${name} to the top of the queue` });
    expect(move).toHaveAttribute('title', 'Move to top');
    expect(within(list).queryByRole('button', { name: 'Move Stacking: Alpha M44 · Ha to the top of the queue' })).toBeNull();
    expect(within(list).queryByRole('button', { name: 'Move Stacking: Alpha M44 · R to the top of the queue' })).toBeNull();
    await userEvent.click(move);
    await waitFor(() => expect(calls).toEqual([{ id: 'G', body: { position: 0 } }]));
    await waitFor(() => expect(move.closest('li')).toHaveTextContent('queued · 1 · next'));
    expect(move).toBeDisabled();
    expect(within(list).getByRole('group', { name: 'Automatic' })).toHaveTextContent('queued · 2 · 1 ahead');
    expect(within(list).getAllByRole('listitem')[0]).toHaveTextContent('Alpha M44 · R');
    expect(within(list).getAllByRole('listitem')[0]).toHaveAttribute('data-job-state', 'running');
  });

  it('moves a queued WBPP run to the top across databases and disables the task already next', async () => {
    const running = row({
      key: 'wbpp:askar:1', kind: 'wbpp', title: 'WBPP', scope: 'Askar · Running project',
      detail: 'Running in PixInsight · 1 steps done', queue: 'wbpp', stoppable: true,
      control: { kind: 'wbpp-running', db_id: 'askar' },
    });
    const queued = (n: number, position: number): JobRow => {
      const dbId = n === 2 ? 'other' : 'askar';
      return row({
        key: `wbpp-queued:q${n}`, kind: 'wbpp', title: 'WBPP',
        scope: `${n === 2 ? 'Other catalog' : 'Askar'} · Project ${n}`, detail: 'Waiting for the run ahead',
        status: 'queued', queue: 'wbpp', position, stoppable: true,
        control: { kind: 'wbpp-queued', db_id: dbId, queue_id: `q${n}` },
      });
    };
    let rows = [running, queued(1, 0), queued(2, 1), queued(3, 2)];
    const calls: unknown[] = [];
    mockJobs(() => rows);
    server.use(
      http.post('/api/wbpp/queue/:queueId/move', async ({ params, request }) => {
        calls.push({ id: params.queueId, body: await request.json() });
        rows = [running, queued(3, 0), queued(1, 1), queued(2, 2)];
        return ok({ running: [], queued: [] });
      }),
    );
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(within(list).getByRole('button', { name: 'Move WBPP: Askar · Project 1 to the top of the queue' })).toBeDisabled();
    await userEvent.click(within(list).getByRole('button', { name: 'Move WBPP: Askar · Project 3 to the top of the queue' }));
    await waitFor(() => expect(calls).toEqual([{ id: 'q3', body: { position: 0 } }]));
    await waitFor(() => expect(within(list).getAllByRole('listitem')[1]).toHaveTextContent('Askar · Project 3'));
    expect(within(list).getAllByRole('listitem')[1]).toHaveTextContent('queued · 1 · next');
    expect(within(list).getAllByRole('listitem')[0]).toHaveTextContent('Askar · Running project');
  });

  it('does not offer queue controls to a read-only user', async () => {
    mockJobs(() => [stackRow('R', 'running', 1, 4), stackRow('B', 'queued', 0, 0, 1)]);
    renderChip({ canWrite: false });
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Alpha M44 · B');
    expect(within(list).queryByRole('button', { name: /to the top of the queue/ })).toBeNull();
  });

  it("moves a person's build past an automatic one queued ahead of it, across the headings", async () => {
    const calls: string[] = [];
    mockJobs(() => [
      stackRow('R', 'running', 1, 4),
      stackRow('B', 'queued', 0, 0, 1),
      stackRow('Ha', 'queued', 0, 0, 0, true),
    ]);
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
    const transfer = (id: string, state: 'paused' | 'review', automatic: boolean) => ({
      id, connection_id: 'c', revision: 1, state, catalog: 'Askar', label: `M44 ${id}`, total_files: 4,
      completed_files: 1, total_bytes: 4, completed_bytes: 1, current_file: '', sent_bytes: 0,
      error: state === 'review' ? 'Incomplete calibration' : null, next_attempt_ms: 0, warnings: null,
      manifest: null, automatic,
    });
    mockJobs(() => [
      stackRow('R', 'running', 1, 4),
      row({
        key: 'transfer:held', kind: 'transfer', title: 'Transfers', scope: 'Askar · M44 held',
        detail: 'Incomplete calibration', status: 'attention', state_label: 'Review required',
        automatic: true, background: true, transfer: transfer('held', 'review', true),
      }),
      row({
        key: 'transfer:paused', kind: 'transfer', title: 'Transfers', scope: 'Askar · M44 paused',
        detail: '1/4 files · 0.0 MiB', status: 'waiting', state_label: 'Paused',
        automatic: true, background: true, transfer: transfer('paused', 'paused', true),
      }),
    ]);
    renderChip();
    const chip = await screen.findByRole('button', { name: /Background jobs/ });
    await waitFor(() => expect(counts(chip)).toEqual(['1 running', '1 waiting', '1 needs attention']));
    expect(chip).toHaveAccessibleName('Background jobs: 25% done, 1 running, 1 waiting, 1 needs attention');
    await userEvent.click(chip);
    const automatic = within(await screen.findByRole('region', { name: 'Background jobs' }))
      .getByRole('group', { name: 'Automatic' });
    expect(counts(automatic.querySelector('.activity-group-head')!)).toEqual(['1 waiting', '1 needs attention']);
    const rows = within(automatic).getAllByRole('listitem');
    expect(rows.map((item) => item.getAttribute('data-job-state'))).toEqual(['attention', 'waiting']);
    expect(rows[0]).toHaveTextContent('Review required');
  });

  it('shows WBPP runs, and gives their controls to an editor without database management', async () => {
    mockJobs(() => [row({
      key: 'wbpp:askar:1', kind: 'wbpp', title: 'WBPP', scope: 'Askar · project Bubble',
      detail: 'Running in PixInsight · Calibration · 2 steps done', queue: 'wbpp', stoppable: true,
      control: { kind: 'wbpp-running', db_id: 'askar' },
    })]);
    server.use(http.get('/api/info', () => ok({ version: 'test', allow_database_management: false })));
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Askar · project Bubble');
    expect(list).toHaveTextContent('Running in PixInsight · Calibration · 2 steps done');
    // A run uses the folders the PixInsight settings define: an editor's.
    expect(await within(list).findByRole('button', { name: 'Stop WBPP: Askar · project Bubble' })).toBeInTheDocument();
  });

  it('shows the chip once a job appears on a quiet server', async () => {
    let building = false;
    mockJobs(() => (building ? [stackRow('R', 'running', 1, 3, null, true)] : []));
    const view = renderChip();
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    // Nothing runs yet: no chip at all.
    expect(view.container.querySelector('.activity-chip')).toBeNull();

    building = true;
    const chip = await screen.findByRole('button', { name: /Background jobs/ }, { timeout: 7000 });
    expect(counts(chip)).toEqual(['1 running']);
  }, 10_000);

  it('lists a processing run beside the build it waits for, then its progress, and links to its stacks', async () => {
    let running = false;
    const blur = (): JobRow => row({
      key: `stack:${'b'.repeat(64)}`, kind: 'processing', title: 'BlurXTerminator', scope: 'Alpha M44 · R',
      detail: running ? '' : 'Waiting for the stack build of Alpha M44 · Ha',
      status: running ? 'running' : 'queued', percent: running ? 42 : null, queue: 'stack',
      position: running ? undefined : 0, control: { kind: 'stack', job_id: 'b'.repeat(64) },
      stoppable: true, started_at: running ? 1_760_000_000 : undefined,
      link: { view: 'stacks', db: 'askar', project: 1, target: 42 },
    });
    mockJobs(() => (running
      ? [blur()]
      : [{ ...stackRow('Ha', 'running', 3, 10), link: { view: 'stacks', db: 'askar', project: 1, target: 42 } }, blur()]));
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
    const item = within(list).getByRole('listitem');
    expect(item).toHaveTextContent('42%');
    expect(item).toHaveTextContent('BlurXTerminator');
    // The detail shows when the run started, in local time.
    expect(item).toHaveTextContent('started');
    expect(within(item).getByRole('button', { name: 'Stop BlurXTerminator: Alpha M44 · R' })).toBeInTheDocument();
    await userEvent.click(within(item).getByRole('link', { name: 'Go to BlurXTerminator: Alpha M44 · R' }));
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent(
      '/stacks?db=askar&project=1&target=42'
    );
  }, 10_000);

  it('says what a running color composition is doing, and offers Stop', async () => {
    mockJobs(() => [{
      ...stackRow('C', 'running', 125, 400),
      title: 'Composing color',
      detail: 'H-alpha: BlurXTerminator (42%)',
    }]);
    renderChip();
    await userEvent.click(await screen.findByRole('button', { name: /Background jobs/ }));
    const list = await screen.findByRole('region', { name: 'Background jobs' });
    expect(list).toHaveTextContent('Composing color');
    expect(list).toHaveTextContent('H-alpha: BlurXTerminator (42%)');
    expect(within(list).getByRole('button', { name: /^Stop / })).toBeInTheDocument();
  });
});
