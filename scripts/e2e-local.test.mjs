import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  breakStaleLock,
  defaultPorts,
  isAlive,
  ownedGroups,
  parseArgs,
  parsePs,
  readOwner,
  releaseLock,
  stuckBrowsers,
  tryAcquire,
} from './e2e-local.mjs';

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), 'e2e-local.mjs');

const scratchDirs = [];
function scratch() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'psf-guard-e2e-local-test-'));
  scratchDirs.push(dir);
  return dir;
}
test.after(() => {
  for (const dir of scratchDirs) fs.rmSync(dir, { recursive: true, force: true });
});

/** A pid that has just exited. */
function deadPid() {
  const child = spawnSync(process.execPath, ['-e', 'console.log(process.pid)'], { encoding: 'utf8' });
  return Number(child.stdout.trim());
}

/** Start the runner with a fake Playwright; resolves with its output. */
function runScript(args, { lock, runner, env = {} }) {
  const child = spawn(process.execPath, [script, ...args], {
    env: {
      ...process.env,
      PSF_GUARD_E2E_LOCK: lock,
      PSF_GUARD_E2E_BINARY: process.execPath,
      PSF_GUARD_E2E_LOCAL_RUNNER: runner,
      ...env,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  child.stdout.on('data', (chunk) => (output += chunk));
  child.stderr.on('data', (chunk) => (output += chunk));
  const done = new Promise((resolve) => child.on('exit', (code) => resolve({ code, output })));
  return { child, done, output: () => output };
}

// The runner needs POSIX process groups and refuses to run on Windows.
const posixOnly = { skip: process.platform === 'win32' && 'needs POSIX process groups' };

async function until(predicate, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return predicate();
}

test('finds only Playwright browsers in the exiting state', () => {
  const processes = parsePs(
    [
      '  501     1   501 ?Es  Playwright',
      '  502     1   502 ?E   /Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1200/chrome-headless-shell-mac-arm64/chrome-headless-shell',
      '  503     1   503 S    /Users/x/Library/Caches/ms-playwright/webkit-2200/Playwright.app/Contents/MacOS/Playwright',
      '  504     1   504 ?E   /usr/bin/some-daemon',
      '  505   400   400 Ss+  chrome-headless',
    ].join('\n'),
  );
  assert.deepEqual(
    stuckBrowsers(processes).map((p) => p.pid),
    [501, 502],
  );
});

test('owns its group and the groups its descendants lead, and nothing else', () => {
  const processes = parsePs(
    [
      '100   1 100 Ss node playwright', // the runner
      '101 100 100 S  node worker', // in the runner's group
      '102 100 102 Ss sh -c psf-guard', // web server, its own group
      '103 102 102 S  psf-guard',
      '104 100 104 Ss chrome-headless-shell',
      '200   1 200 Ss another-agent-run',
      '201 200 200 S  psf-guard',
    ].join('\n'),
  );
  assert.deepEqual([...ownedGroups(processes, 100)].sort(), [100, 102, 104]);
});

test('parses options and passes everything else to Playwright', () => {
  const options = parseArgs(['--director', '--no-wait', '--idle', '90s', 'e2e/a.spec.ts', '--timeout', '5000'], {});
  assert.equal(options.director, true);
  assert.equal(options.wait, false);
  assert.equal(options.idle, 90_000);
  assert.equal(options.maxTime, 30 * 60_000);
  assert.deepEqual(options.playwrightArgs, ['e2e/a.spec.ts', '--timeout', '5000']);
  assert.deepEqual(parseArgs(['--', '--idle', '1'], {}).playwrightArgs, ['--idle', '1']);
});

test('derives the same ports for a worktree every time', () => {
  assert.deepEqual(defaultPorts('/a/worktree'), defaultPorts('/a/worktree'));
  const { main, director } = defaultPorts('/a/worktree');
  assert.ok(main >= 20_000 && director < 24_000 && director === main + 3);
});

test('the lock admits one owner and breaks only a dead one', posixOnly, () => {
  const lock = path.join(scratch(), 'lock');
  const live = { pid: process.pid, worktree: '/live', started: new Date().toISOString() };
  assert.equal(tryAcquire(lock, live), true);
  assert.equal(tryAcquire(lock, { ...live, worktree: '/other' }), false);
  assert.equal(breakStaleLock(lock, live), false, 'a live owner keeps its lock');
  assert.equal(readOwner(lock).worktree, '/live');

  releaseLock(lock, { ...live, started: 'someone else' });
  assert.ok(fs.existsSync(lock), 'only the owner releases');
  releaseLock(lock, live);
  assert.ok(!fs.existsSync(lock));

  const dead = { pid: deadPid(), worktree: '/dead', started: new Date().toISOString() };
  assert.equal(isAlive(dead.pid), false);
  tryAcquire(lock, dead);
  assert.equal(breakStaleLock(lock, { ...dead, started: 'an older claim' }), false, 'owner changed since it was read');
  assert.equal(breakStaleLock(lock, dead), true);
  assert.ok(!fs.existsSync(lock));
});

test('a second run waits for the first, and --no-wait refuses', posixOnly, async () => {
  const lock = path.join(scratch(), 'lock');
  const first = runScript([], { lock, runner: 'echo first-start; sleep 2; echo first-end' });
  assert.ok(await until(() => first.output().includes('first-start')));

  const refused = await runScript(['--no-wait'], { lock, runner: 'echo never' }).done;
  assert.equal(refused.code, 75);
  assert.match(refused.output, /another e2e run holds/);
  assert.doesNotMatch(refused.output, /never/);

  const second = runScript([], { lock, runner: 'echo second-ran' });
  assert.ok(await until(() => second.output().includes('waiting for')));
  const [a, b] = await Promise.all([first.done, second.done]);
  assert.equal(a.code, 0);
  assert.equal(b.code, 0);
  assert.ok(b.output.indexOf('waiting for') < b.output.indexOf('second-ran'));
  assert.ok(!fs.existsSync(lock), 'the lock is released');
});

test('a lock left by a dead run is broken', posixOnly, async () => {
  const lock = path.join(scratch(), 'lock');
  tryAcquire(lock, { pid: deadPid(), worktree: '/gone', started: new Date().toISOString() });
  const result = await runScript([], { lock, runner: 'echo ran' }).done;
  assert.equal(result.code, 0);
  assert.match(result.output, /broke a stale lock from \/gone/);
  assert.match(result.output, /ran/);
});

test('the watchdog stops every process group the run created', posixOnly, async () => {
  const lock = path.join(scratch(), 'lock');
  // The fake runner starts a child in its own group from a parent that stays
  // up, as Playwright does with browsers and web servers, prints the pids,
  // and goes quiet.
  const leader =
    "const c=require('child_process').spawn('sleep',['600'],{detached:true,stdio:'ignore'});" +
    "require('fs').writeFileSync(process.argv[1],String(c.pid));setInterval(()=>{},1e6)";
  const runner = [
    'f=$(mktemp)',
    'sleep 600 &',
    'quiet=$!',
    `${JSON.stringify(process.execPath)} -e "${leader}" "$f" &`,
    'while [ ! -s "$f" ]; do sleep 0.1; done',
    'echo "pids $$ $quiet $! $(cat "$f")"; rm -f "$f"',
    'sleep 600',
  ].join('\n');
  const result = await runScript(['--idle', '3s', '--grace', '1s'], { lock, runner }).done;
  assert.equal(result.code, 124);
  assert.match(result.output, /watchdog stopped the run/);
  const pids = /pids (\d+) (\d+) (\d+) (\d+)/.exec(result.output).slice(1).map(Number);
  assert.ok(await until(() => pids.every((pid) => !isAlive(pid)), 5_000), `still alive: ${pids.filter(isAlive)}`);
  assert.ok(!fs.existsSync(lock));
});

test('the hard limit stops a run that keeps printing', posixOnly, async () => {
  const lock = path.join(scratch(), 'lock');
  const runner = 'while :; do echo tick; sleep 0.2; done';
  const result = await runScript(['--max-time', '1500ms', '--grace', '1s'], { lock, runner }).done;
  assert.equal(result.code, 124);
  assert.match(result.output, /timed out after/);
});

test('refuses to start without a server binary', posixOnly, async () => {
  const lock = path.join(scratch(), 'lock');
  const result = await runScript([], {
    lock,
    runner: 'echo never',
    env: { PSF_GUARD_E2E_BINARY: '/nonexistent/psf-guard' },
  }).done;
  assert.equal(result.code, 2);
  assert.match(result.output, /no server binary/);
});
