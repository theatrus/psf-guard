import type { JobControl, JobLink, JobRow, PreviewKind } from '../../api/types';
import type { TransferJob } from '../../api/transferTypes';

/** The route a row's Stop and move buttons call. */
export type ActivityControl =
  | { kind: 'stack'; jobId: string }
  | { kind: 'flats'; dbId: string }
  | { kind: 'sites'; dbId: string }
  | { kind: 'duplicates'; dbId: string }
  | { kind: 'rejects'; dbId: string }
  | { kind: 'backup' }
  | { kind: 'ts-sync'; jobId: string }
  | { kind: 'wbpp-running'; dbId: string }
  | { kind: 'wbpp-queued'; dbId: string; queueId: string }
  | { kind: 'preview'; preview: PreviewKind; dbId: string }
  | { kind: 'import'; dbId: string }
  | { kind: 'export'; dbId: string };

/** The two lines a person can reorder. Each runs one job at a time. */
export type ActivityQueue = 'stack' | 'wbpp';

/**
 * Where a job stands. `attention`: stopped until a person acts, such as a
 * transfer to review. `running`: at work now. `queued`: in a line a person
 * can reorder. `waiting`: work paused for other work, such as automatic
 * work behind a build a person started, or a transfer that is paused or
 * waiting for its retry.
 */
export type ActivityStatus = 'attention' | 'running' | 'queued' | 'waiting';

/**
 * Who the work is for, as in the server's priority tiers. `person`: work a
 * person started, which runs first (`Interactive`). `automatic`: work the
 * server started, or database-wide work, which pauses for a person's work
 * (`Viewed` and `Background`).
 */
export type ActivityTier = 'person' | 'automatic';

/** One background job in the header's list. */
export interface ActivityItem {
  key: string;
  kind: JobRow['kind'];
  transfer?: TransferJob;
  /** Text for the row's corner in place of the status. */
  stateLabel?: string;
  /** What is happening: "Stacking", "Analyzing quality". */
  title: string;
  /** What it happens to: a database, or a target and channel. */
  scope: string;
  /** The progress in words: "1/3 frames", a stage, a file. */
  detail: string;
  status: ActivityStatus;
  /** What waiting work waits for, when the server reports it: "a stack build". */
  waitingFor?: string;
  /** 0–100, or null when the job reports no progress. Waiting work keeps
   *  the progress it had reached. */
  percent: number | null;
  automatic?: boolean;
  /** Automatic or database-wide work, which runs after anything a person
   *  started: the `automatic` tier. */
  background?: boolean;
  /** The work queued behind the current step, when the server reports it
   *  and the detail does not include it: "1 more rig to check", "another
   *  pass queued", "3 channels to stack". */
  remaining?: string;
  /** The full current path or file, for a tooltip. */
  hint?: string;
  /** The line this job runs in, when it has one. */
  queue?: ActivityQueue;
  /** Place among the queued jobs of its line, 0 next. */
  position?: number;
  control?: ActivityControl;
  /** Whether Stop can end it while it runs. A stretch or deconvolution
   *  run cannot: it finishes in moments. */
  stoppable?: boolean;
  /** The view showing what the job works on, scoped by URL. */
  href?: string;
}

/** The URL of a job's view: `/{view}?db=…&project=…&target=…`, then its
 *  other parameters in order. */
export function linkPath(link: JobLink): string {
  const search = new URLSearchParams({ db: link.db });
  if (link.project != null) search.set('project', String(link.project));
  if (link.target != null) search.set('target', String(link.target));
  for (const [name, value] of link.params ?? []) search.set(name, value);
  return `/${link.view}?${search.toString()}`;
}

function controlOf(control: JobControl): ActivityControl {
  switch (control.kind) {
    case 'stack':
      return { kind: 'stack', jobId: control.job_id };
    case 'ts-sync':
      return { kind: 'ts-sync', jobId: control.job_id };
    case 'backup':
      return { kind: 'backup' };
    case 'wbpp-queued':
      return { kind: 'wbpp-queued', dbId: control.db_id, queueId: control.queue_id };
    case 'preview':
      return { kind: 'preview', preview: control.preview, dbId: control.db_id };
    default:
      return { kind: control.kind, dbId: control.db_id };
  }
}

/** `started 21:40`: when a processing run started, in the browser's local time. */
function startedText(unixSeconds: number): string {
  return `started ${new Date(unixSeconds * 1000).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  })}`;
}

/** The header's item for one row of `GET /api/jobs`. */
export function jobItem(row: JobRow): ActivityItem {
  const detail = row.started_at != null
    ? [row.detail, startedText(row.started_at)].filter(Boolean).join(' · ')
    : row.detail;
  return {
    key: row.key,
    kind: row.kind,
    transfer: row.transfer,
    stateLabel: row.state_label,
    title: row.title,
    scope: row.scope,
    detail,
    status: row.status,
    waitingFor: row.waiting_for,
    percent: row.percent,
    automatic: row.automatic,
    background: row.background,
    remaining: row.remaining,
    hint: row.hint,
    queue: row.queue,
    position: row.position,
    control: row.control ? controlOf(row.control) : undefined,
    stoppable: row.stoppable,
    href: row.link ? linkPath(row.link) : undefined,
  };
}

/** The tier a row belongs to, from what the server reports. */
export function tierOf(item: ActivityItem): ActivityTier {
  return item.background ? 'automatic' : 'person';
}

export interface ActivityGroup {
  tier: ActivityTier;
  /** The heading text. */
  title: string;
  items: ActivityItem[];
  summary: ActivitySummary;
}

export const TIER_TITLES: Record<ActivityTier, string> = {
  person: 'Started by you',
  automatic: 'Automatic',
};

/** The rows under their two headings, each with its counts. A heading with
 *  no rows is left out. The rows keep the server's order. */
export function groupActivity(items: ActivityItem[]): ActivityGroup[] {
  return (['person', 'automatic'] as const)
    .map((tier) => {
      const rows = items.filter((item) => tierOf(item) === tier);
      return { tier, title: TIER_TITLES[tier], items: rows, summary: summarize(rows) };
    })
    .filter((group) => group.items.length > 0);
}

/** How many jobs are queued in each line, for the move buttons' limits. */
export function lineLengths(items: ActivityItem[]): Record<ActivityQueue, number> {
  const lengths: Record<ActivityQueue, number> = { stack: 0, wbpp: 0 };
  for (const item of items) {
    if (item.queue && item.position != null) lengths[item.queue] += 1;
  }
  return lengths;
}

/** A queued row's place in its whole line, under either heading:
 *  `queued · 4 · 3 ahead`, `queued · 1 · next`. */
export function queueText(item: ActivityItem): string {
  if (item.position == null || !item.queue) return 'queued';
  const place = `queued · ${item.position + 1}`;
  return item.position === 0 ? `${place} · next` : `${place} · ${item.position} ahead`;
}

export interface ActivitySummary {
  count: number;
  /** Work stopped until a person acts. */
  attention: number;
  running: number;
  /** Automatic work paused for other work, and paused or retrying transfers. */
  waiting: number;
  /** Jobs in a line a person can reorder, such as builds they requested. */
  queued: number;
  /** The mean progress of the running jobs that report one, or null. */
  percent: number | null;
}

export function summarize(items: ActivityItem[]): ActivitySummary {
  const running = items.filter((item) => item.status === 'running');
  const measured = running.filter((item) => item.percent != null);
  return {
    count: items.length,
    attention: items.filter((item) => item.status === 'attention').length,
    running: running.length,
    waiting: items.filter((item) => item.status === 'waiting').length,
    queued: items.filter((item) => item.status === 'queued').length,
    percent: measured.length > 0
      ? measured.reduce((sum, item) => sum + (item.percent ?? 0), 0) / measured.length
      : null,
  };
}
