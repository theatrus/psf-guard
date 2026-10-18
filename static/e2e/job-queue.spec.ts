import { expect, test } from '@playwright/test';
import type { StackActivityEntry, WbppActivity } from '../src/api/types';
import { registerFixtureDb, resetDatabases } from './helpers';

for (const width of [1280, 390]) {
  for (const queue of ['stack', 'wbpp'] as const) {
    test(`${queue} task moves directly to the top with compact controls at ${width}px`, async ({ page, request }, testInfo) => {
      await resetDatabases(request);
      const db = await registerFixtureDb(request, { slug: 'queue-rig' });
      const ids = ['a', 'b', 'c', 'd'].map(letter => letter.repeat(64));
      let stacks: StackActivityEntry[] = ids.map((id, index) => ({
        kind: index === 3 ? 'processing' : 'mono', job_id: id, database_id: db.id,
        project_id: 1, target_id: 1, state: index === 0 ? 'running' : 'queued',
        label: `Queue target ${index}`, detail: index === 0 ? 'Registering frames' : 'Waiting for the current task',
        processed_units: index === 0 ? 1 : 0, total_units: 4,
        created_unix_seconds: index + 1, queue_position: index === 0 ? null : index - 1,
        automatic: index === 1,
        ...(index === 3 ? { processing: { operation: 'BlurXTerminator', channel: 'R', stoppable: true } } : {}),
      }));
      let wbpp: WbppActivity = {
        running: [{ db_id: db.id, db_name: 'Queue rig', scope: 'Running project', stage: 'running',
          wbpp_stage: 'Calibration', wbpp_steps: 1, started_at: 1 }],
        queued: [1, 2, 3].map(n => ({ id: `q${n}`, db_id: db.id, db_name: 'Queue rig', scope: `Project ${n}`,
          project_id: n, target_id: null, position: n, queued_at: n })),
      };
      const moves: unknown[] = [];
      const stackActivity = () => ({ schema_version: 1, active: queue === 'stack' ? stacks : [] });
      const wbppActivity = () => queue === 'wbpp' ? wbpp : { running: [], queued: [] };
      await page.route('**/api/stack-activity', route => route.fulfill({ json: { success: true, data: stackActivity(), error: null } }));
      await page.route('**/api/wbpp/activity', route => route.fulfill({ json: { success: true, data: wbppActivity(), error: null } }));
      const endpoint = queue === 'stack' ? `/api/stack-activity/${ids[3]}/move` : '/api/wbpp/queue/q3/move';
      await page.route(`**${endpoint}`, async route => {
        moves.push(route.request().postDataJSON());
        if (queue === 'stack') {
          stacks = [stacks[0], { ...stacks[3], queue_position: 0 },
            { ...stacks[1], queue_position: 1 }, { ...stacks[2], queue_position: 2 }];
        } else {
          wbpp = { ...wbpp, queued: [wbpp.queued[2], wbpp.queued[0], wbpp.queued[1]]
            .map((run, index) => ({ ...run, position: index + 1 })) };
        }
        await route.fulfill({ json: { success: true, data: queue === 'stack' ? stackActivity() : wbppActivity(), error: null } });
      });
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/');
      await page.getByRole('button', { name: /Background jobs/ }).click();
      const list = page.getByRole('region', { name: 'Background jobs' });
      const name = queue === 'stack' ? 'BlurXTerminator: Queue target 3' : 'WBPP: Queue rig · Project 3';
      const top = list.getByRole('button', { name: `Move ${name} to the top of the queue` });
      const earlier = list.getByRole('button', { name: `Run ${name} earlier` });
      const later = list.getByRole('button', { name: `Run ${name} later` });
      await expect(top).toHaveAttribute('title', 'Move to top');
      // The new button stays the same small size as the adjacent arrows.
      const sizes = await Promise.all([top, earlier, later].map(button => button.boundingBox()));
      expect(sizes.every(Boolean)).toBe(true);
      expect(sizes[0]!.width).toBeLessThanOrEqual(sizes[1]!.width + 1);
      expect(sizes[0]!.height).toBe(sizes[1]!.height);
      expect(sizes[0]!.height).toBe(sizes[2]!.height);
      expect(await top.locator('svg').getAttribute('aria-hidden')).toBe('true');
      const row = list.getByRole('listitem').filter({
        has: page.getByRole('button', { name: `Move ${name} to the top of the queue` }),
      });
      await expect(row).toContainText('queued · 3 · 2 ahead');
      expect(await list.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
      await list.screenshot({ path: testInfo.outputPath(`move-to-top-${queue}-${width}.png`) });
      await top.click();
      await expect.poll(() => moves).toEqual([{ position: 0 }]);
      await expect(row).toContainText('queued · 1 · next');
      await expect(top).toBeDisabled();
      if (queue === 'stack') {
        const automatic = list.getByRole('group', { name: 'Automatic' });
        await expect(automatic).toContainText('queued · 2 · 1 ahead');
        await expect(automatic.getByRole('button', { name: /to the top of the queue/ })).toHaveCount(0);
      }
      await expect(list.getByRole('listitem').filter({ hasText: queue === 'stack' ? 'Queue target 0' : 'Running project' }))
        .toHaveAttribute('data-job-state', 'running');
    });
  }
}
