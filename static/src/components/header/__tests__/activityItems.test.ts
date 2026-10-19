import { describe, expect, it } from 'vitest';
import type { JobRow } from '../../../api/types';
import {
  groupActivity,
  jobItem,
  lineLengths,
  linkPath,
  queueText,
  summarize,
  tierOf,
} from '../activityItems';

/** A row as `GET /api/jobs` returns it. The server builds the text and
 *  orders the rows; `src/server/jobs/tests.rs` covers both. */
function row(overrides: Partial<JobRow> & Pick<JobRow, 'key'>): JobRow {
  return {
    kind: 'stack', title: 'Stacking', scope: `M44 · ${overrides.key}`, detail: '', status: 'running',
    background: false, automatic: false, percent: null, stoppable: false, ...overrides,
  };
}

function queued(key: string, position: number, background = false): JobRow {
  return row({
    key, status: 'queued', queue: 'stack', position, background, automatic: background,
    control: { kind: 'stack', job_id: key },
  });
}

describe('header items from the jobs route', () => {
  it('copies the row and turns its control into the client shape', () => {
    const item = jobItem(row({
      key: 'stack:R', detail: '3/12 frames', percent: 25, waiting_for: 'a stack build',
      remaining: '2 channels to stack', hint: 'registering', queue: 'stack', position: 1,
      stoppable: true, control: { kind: 'stack', job_id: 'R' }, state_label: 'Uploading',
    }));
    expect(item).toMatchObject({
      key: 'stack:R', kind: 'stack', title: 'Stacking', scope: 'M44 · stack:R', detail: '3/12 frames',
      status: 'running', percent: 25, waitingFor: 'a stack build', remaining: '2 channels to stack',
      hint: 'registering', queue: 'stack', position: 1, stoppable: true, stateLabel: 'Uploading',
      control: { kind: 'stack', jobId: 'R' },
    });
    expect(item.href).toBeUndefined();
    expect(jobItem(row({ key: 'f', control: { kind: 'flats', db_id: 'astro' } })).control)
      .toEqual({ kind: 'flats', dbId: 'astro' });
    expect(jobItem(row({ key: 'b', control: { kind: 'backup' } })).control).toEqual({ kind: 'backup' });
    expect(jobItem(row({ key: 't', control: { kind: 'ts-sync', job_id: 'import:a' } })).control)
      .toEqual({ kind: 'ts-sync', jobId: 'import:a' });
    expect(jobItem(row({ key: 'w', control: { kind: 'wbpp-queued', db_id: 'a', queue_id: 'q4' } })).control)
      .toEqual({ kind: 'wbpp-queued', dbId: 'a', queueId: 'q4' });
  });

  it('builds each link as a view URL with the database, project, target and parameters in order', () => {
    expect(linkPath({ view: 'grid', db: 'db-a' })).toBe('/grid?db=db-a');
    expect(linkPath({ view: 'stacks', db: 'db-a', project: 1, target: 7 })).toBe('/stacks?db=db-a&project=1&target=7');
    expect(linkPath({ view: 'sequence', db: 'db-a', project: 4, target: 42, params: [['filterName', 'Ha']] }))
      .toBe('/sequence?db=db-a&project=4&target=42&filterName=Ha');
    expect(linkPath({ view: 'calibration', db: 'astro', params: [['calKind', 'flat'], ['calRig', 'c925']] }))
      .toBe('/calibration?db=astro&calKind=flat&calRig=c925');
    expect(jobItem(row({ key: 'l', link: { view: 'stacks', db: 'db-a', project: 3 } })).href)
      .toBe('/stacks?db=db-a&project=3');
  });

  it('adds when a processing run started, in local time, to its detail', () => {
    const startedAt = new Date(1_760_000_000_000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    expect(jobItem(row({ key: 'p', kind: 'processing', detail: '', started_at: 1_760_000_000 })).detail)
      .toBe(`started ${startedAt}`);
    expect(jobItem(row({ key: 'p', kind: 'processing', detail: 'Reading the stack', started_at: 1_760_000_000 })).detail)
      .toBe(`Reading the stack · started ${startedAt}`);
    expect(jobItem(row({ key: 'p', detail: 'Waiting for a stack build' })).detail).toBe('Waiting for a stack build');
  });

  it('files a row by its tier', () => {
    expect(tierOf(jobItem(row({ key: 'a' })))).toBe('person');
    expect(tierOf(jobItem(row({ key: 'b', background: true })))).toBe('automatic');
  });
});

describe('the job list by tier', () => {
  it('groups the rows in the server order under their headings, each with its counts', () => {
    const items = [
      row({ key: 'held', kind: 'transfer', title: 'Transfers', status: 'attention' }),
      row({ key: 'run', percent: 25 }),
      queued('first', 0),
      row({ key: 'refresh', title: 'Refreshing catalog', background: true, percent: 50 }),
      queued('auto', 1, true),
      row({ key: 'scan', title: 'Analyzing database quality', status: 'waiting', background: true, percent: 30 }),
    ].map(jobItem);
    const groups = groupActivity(items);
    expect(groups.map((group) => group.title)).toEqual(['Started by you', 'Automatic']);
    expect(groups[0].items.map((item) => item.key)).toEqual(['held', 'run', 'first']);
    expect(groups[0].summary).toMatchObject({ attention: 1, running: 1, queued: 1, waiting: 0 });
    expect(groups[1].items.map((item) => item.key)).toEqual(['refresh', 'auto', 'scan']);
    expect(groups[1].summary).toMatchObject({ running: 1, queued: 1, waiting: 1 });
    // The flat list is the groups in turn, and the chip counts them all.
    expect(items).toEqual([...groups[0].items, ...groups[1].items]);
    // Waiting work's progress does not count toward the chip's.
    expect(summarize(items)).toEqual({ count: 6, attention: 1, running: 2, waiting: 1, queued: 2, percent: 37.5 });
    expect(summarize([])).toEqual({ count: 0, attention: 0, running: 0, waiting: 0, queued: 0, percent: null });
  });

  it("shows a queued row's place in the whole line, under either heading", () => {
    const items = [row({ key: 'run' }), queued('mine', 1), queued('refresh', 0, true)].map(jobItem);
    const [person, automatic] = groupActivity(items);
    const mine = person.items.find((item) => item.key === 'mine')!;
    const refreshBuild = automatic.items.find((item) => item.key === 'refresh')!;
    expect(queueText(mine)).toBe('queued · 2 · 1 ahead');
    expect(queueText(refreshBuild)).toBe('queued · 1 · next');
    // The move buttons work on the whole line.
    expect(lineLengths(items)).toEqual({ stack: 2, wbpp: 0 });
    expect(queueText({ ...mine, queue: undefined })).toBe('queued');
  });

  it('counts each line, and leaves out a heading with nothing under it', () => {
    const items = [
      queued('a', 0), queued('b', 1),
      row({ key: 'w', kind: 'wbpp', title: 'WBPP', status: 'queued', queue: 'wbpp', position: 0 }),
    ].map(jobItem);
    expect(lineLengths(items)).toEqual({ stack: 2, wbpp: 1 });
    expect(groupActivity(items).map((group) => group.tier)).toEqual(['person']);
    expect(groupActivity([])).toEqual([]);
  });
});
