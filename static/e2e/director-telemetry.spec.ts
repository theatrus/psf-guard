import { expect, test } from '@playwright/test';

for (const width of [1440, 390]) {
  test(`completed operation replay stays separate from live status at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 1000 });
    const now = Date.now();
    await page.route('**/api/director/v1/rigs/status', route => route.fulfill({ json: {
      success: true, error: null, status: 'ready', data: [{
        rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'Simulator', revision: 1 },
        catalog_slug: 'telemetry-test', catalog_name: 'Director simulator',
        status: { rig_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', session_id: 'session', reported_at_ms: now - 120000,
          received_at_ms: now - 120000, payload: { phase: 'completed', safety: 'Safe', operation: 'Autofocus', operation_elapsed_ms: 90000 } },
        status_age_ms: 120000, status_stale: true, checkins: [], contacts: { program_pull: null, check_in: { at_ms: now, detail: 'ledger' }, status: { at_ms: now - 120000, detail: 'session' } },
        connectivity: { state: 'online', last_contact_ms: now, age_ms: 0 }, assignments: [], pending_receipts: 0,
        recent_operations: [{ received_at_ms: now, event: { ledger_id: 'ledger', sequence: 1, preparation_id: 'prep', event: { kind: 'completed', observation: {
          command: { target_id: 'target', operation: { operation: 'before_target' } },
          completion: { ended_at_ms: now - 3600000, elapsed_ms: 75000, outcome: { outcome: 'succeeded' } },
        } } } }],
      }],
    } }));
    await page.goto('/#/sky?live=1');
    const live = page.getByRole('region', { name: 'Live rigs' });
    await expect(live).toContainText('completed, Autofocus for 2 min, safety Safe (stale');
    // An empty disposable server opens first-run settings over the page.
    if (await page.getByRole('heading', { name: 'PSF Guard Settings' }).isVisible())
      await page.getByRole('button', { name: '×', exact: true }).click();
    await live.getByText('Completed operations (1)', { exact: true }).click();
    await expect(live).toContainText('before target: succeeded, 75 s');
    await expect(live).toContainText('completed 1 h ago');
    await expect(live).toContainText(/received \d+ s ago/);
    await live.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath(`telemetry-${width}.png`), fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
