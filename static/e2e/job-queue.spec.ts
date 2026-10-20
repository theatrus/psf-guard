import { expect, test } from '@playwright/test';
import type { JobRow } from '../src/api/types';
import { registerFixtureDb, resetDatabases } from './helpers';
import { jobRow, stubJobs } from './jobRows';

for (const width of [1280, 390]) {
  for (const queue of ['stack', 'wbpp'] as const) {
    test(`${queue} task moves directly to the top with compact controls at ${width}px`, async ({ page, request }, testInfo) => {
      await resetDatabases(request);
      const db = await registerFixtureDb(request, { slug: 'queue-rig' });
      const ids = ['a', 'b', 'c', 'd'].map(letter => letter.repeat(64));
      // The stack line: one build running, an automatic build, a build and a
      // processing run queued. Positions are places in the whole line.
      const stackRow = (index: number, position: number | null): JobRow => jobRow({
        key: `stack:${ids[index]}`, kind: index === 3 ? 'processing' : 'stack',
        title: index === 3 ? 'BlurXTerminator' : 'Stacking', scope: `Queue target ${index}`,
        detail: index === 0 ? '1/4 frames' : 'Waiting for the build ahead',
        status: index === 0 ? 'running' : 'queued', percent: index === 0 ? 25 : null,
        queue: 'stack', position: position ?? undefined, stoppable: true,
        automatic: index === 1, background: index === 1,
        control: { kind: 'stack', job_id: ids[index] },
        link: { view: 'stacks', db: db.id, project: 1, target: 1 },
      });
      let stackPositions = [null, 0, 1, 2];
      const stackRows = () => {
        const rows = [0, 1, 2, 3].map((index) => stackRow(index, stackPositions[index]));
        const person = rows.filter((row) => !row.background);
        // A person's queued rows in line order, then the automatic build.
        return [person[0], ...person.slice(1).sort((a, b) => a.position! - b.position!), rows[1]];
      };
      const wbppRow = (n: number, position: number): JobRow => jobRow({
        key: `wbpp-queued:q${n}`, kind: 'wbpp', title: 'WBPP', scope: `Queue rig · Project ${n}`,
        detail: 'Waiting for the run ahead', status: 'queued', queue: 'wbpp', position, stoppable: true,
        control: { kind: 'wbpp-queued', db_id: db.id, queue_id: `q${n}` },
        link: { view: 'stacks', db: db.id, project: n },
      });
      const running = jobRow({
        key: `wbpp:${db.id}:1`, kind: 'wbpp', title: 'WBPP', scope: 'Queue rig · Running project',
        detail: 'Running in PixInsight · Calibration · 1 steps done', queue: 'wbpp', stoppable: true,
        control: { kind: 'wbpp-running', db_id: db.id },
      });
      let wbppOrder = [1, 2, 3];
      const wbppRows = () => [running, ...wbppOrder.map((n, index) => wbppRow(n, index))];
      const moves: unknown[] = [];
      await stubJobs(page, () => (queue === 'stack' ? stackRows() : wbppRows()));
      const endpoint = queue === 'stack' ? `/api/stack-activity/${ids[3]}/move` : '/api/wbpp/queue/q3/move';
      await page.route(`**${endpoint}`, async route => {
        moves.push(route.request().postDataJSON());
        if (queue === 'stack') stackPositions = [null, 1, 2, 0];
        else wbppOrder = [3, 1, 2];
        await route.fulfill({ json: { success: true, data: queue === 'stack' ? { schema_version: 1, active: [] } : { running: [], queued: [] }, error: null } });
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
