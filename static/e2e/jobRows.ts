import type { Page } from '@playwright/test';
import type { DatabaseJobs, JobRow } from '../src/api/types';

/**
 * Stubs for the header's job list (`GET /api/jobs`). A spec that holds the
 * server at a moment real work would take minutes to reach serves the rows
 * the server builds at that moment, in the server's order.
 */

/** A row with the server's defaults: a person's running work. */
export function jobRow(overrides: Partial<JobRow> & Pick<JobRow, 'key' | 'title' | 'scope'>): JobRow {
  return {
    kind: 'stack', detail: '', status: 'running', background: false, automatic: false,
    percent: null, stoppable: false, ...overrides,
  };
}

/** Serve `rows()` and `databases()` from `GET /api/jobs` on each poll. */
export async function stubJobs(
  page: Page,
  rows: () => JobRow[],
  databases: () => DatabaseJobs[] = () => []
): Promise<void> {
  await page.route('**/api/jobs', (route) => route.fulfill({
    json: { success: true, data: { jobs: rows(), databases: databases() }, error: null, status: 'ready' },
  }));
}
