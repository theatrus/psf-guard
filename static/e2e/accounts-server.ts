import highlights from '../src/updates/highlights.json' with { type: 'json' };
import { expect, type Browser, type Page } from '@playwright/test';
import { spawn, spawnSync, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { fixtureDbPath, fixtureImageDir, tmpBase } from './helpers';

// A server with user accounts on a copy of the fixture catalog. The shared
// servers have no accounts, so specs about one user's preferences start
// their own: a viewer, alice, and an editor, bob.

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BINARY = process.env.PSF_GUARD_E2E_BINARY
  ?? path.resolve(__dirname, '../../target/release/psf-guard');
export const ACCOUNT_PASSWORD = 'a-long-e2e-password';

export interface AccountsServer {
  base: string;
  /** The fixture catalog's database slug, known after the first sign-in. */
  dbId(): string;
  /** A browser with nothing of its own yet, signed in as `name`. */
  signIn(browser: Browser, name: 'alice' | 'bob'): Promise<Page>;
  stop(): void;
}

/** Start one, `offset` ports above the main server. Each spec that needs
 *  one picks its own offset. */
export async function startAccountsServer(name: string, offset: number): Promise<AccountsServer> {
  const port = Number(process.env.PSF_GUARD_E2E_PORT ?? 13099) + offset;
  const base = `http://127.0.0.1:${port}`;
  const directory = path.join(tmpBase(), `${name}-${process.pid}`);
  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  const catalog = path.join(directory, 'scheduler.sqlite');
  fs.copyFileSync(fixtureDbPath(), catalog);
  const registry = path.join(directory, 'registry.json');
  const passwordFile = path.join(directory, 'password');
  fs.writeFileSync(passwordFile, `${ACCOUNT_PASSWORD}\n`);
  // A viewer, to show a read-only account saves its own view, and an editor.
  for (const [user, role] of [['alice', 'read-only'], ['bob', 'read-write']]) {
    const added = spawnSync(BINARY, [
      'users', 'add', user, '--role', role, '--password-file', passwordFile, '--registry', registry,
    ], { encoding: 'utf8' });
    expect(added.status, added.stderr).toBe(0);
  }
  const server: ChildProcess = spawn(BINARY, [
    'server', catalog, fixtureImageDir(),
    '--registry', registry,
    '--cache-dir', path.join(directory, 'cache'),
    '--host', '127.0.0.1',
    '--port', String(port),
  ], { stdio: 'ignore' });

  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      const response = await fetch(`${base}/api/auth/status`);
      if (response.ok) break;
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) {
      server.kill();
      throw new Error('The accounts server did not start');
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  let dbId = '';
  return {
    base,
    dbId: () => dbId,
    async signIn(browser, user) {
      const context = await browser.newContext({
        storageState: {
          cookies: [],
          origins: [{
            origin: base,
            localStorage: [{ name: 'psf-guard.whats-new.acknowledged', value: highlights[0].version }],
          }],
        },
      });
      const page = await context.newPage();
      await page.goto(`${base}/`);
      await page.getByLabel('Username').fill(user);
      await page.getByLabel('Password').fill(ACCOUNT_PASSWORD);
      // Signing in reloads the page once the sign-in screen has gone. Wait
      // for that load, so the caller's next navigation is not cut short.
      await Promise.all([
        page.waitForEvent('load'),
        page.getByRole('button', { name: 'Sign in' }).click(),
      ]);
      await expect(page.getByRole('heading', { name: 'Sign in to PSF Guard' })).toHaveCount(0);
      if (!dbId) {
        dbId = await page.evaluate(async () => {
          const response = await fetch('/api/databases');
          const body = await response.json();
          return body.data[0].id as string;
        });
      }
      return page;
    },
    stop: () => {
      server.kill();
    },
  };
}
