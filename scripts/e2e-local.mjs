#!/usr/bin/env node
// Run the Playwright e2e suite on a developer machine without wedging it.
//
// One run at a time per machine, behind a lock in /tmp. The run refuses to
// start while browsers are stuck exiting, lives in its own process group,
// and is stopped on a hard time limit, an output watchdog, or Ctrl-C. A stop
// signals only the process groups this run created: never a `pkill` by name.
//
// Usage: scripts/e2e-local.mjs [options] [--] [playwright test args...]
// See static/README.md, "Running e2e locally".

import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SCRIPT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TAG = 'e2e-local:';

/** Fixed, machine-wide: TMPDIR differs between sandboxes and sessions. */
export const DEFAULT_LOCK = '/tmp/psf-guard-e2e.lock';

export const EXIT = { busy: 75, refused: 2, stopped: 124, interrupted: 130 };

const USAGE = `Usage: scripts/e2e-local.mjs [options] [--] [playwright test args...]

Runs \`playwright test\` in static/ for this worktree, one run per machine.

Options:
  --director        use playwright.director.config.ts
  --no-wait         fail at once if another run holds the lock
  --max-time <d>    hard limit for the whole run (default 30m)
  --idle <d>        stop after this long without output (default 5m)
  --grace <d>       wait between SIGTERM and SIGKILL (default 10s)
  -h, --help        show this help

Durations take ms, s, m or h; a bare number is minutes.

Environment:
  PSF_GUARD_E2E_BINARY          server binary (default target/release/psf-guard)
  PSF_GUARD_E2E_PORT            first of three main-suite ports
  PSF_GUARD_DIRECTOR_E2E_PORT   Director suite port
  PSF_GUARD_E2E_MAX_TIME, PSF_GUARD_E2E_IDLE   defaults for the limits

Exit status: Playwright's own, ${EXIT.stopped} after a time limit or the
watchdog, ${EXIT.interrupted} after Ctrl-C, ${EXIT.busy} when --no-wait finds the
lock held, ${EXIT.refused} when the run refuses to start.`;

export function parseDuration(text, label) {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/.exec(String(text).trim());
  if (!match) throw new Error(`${label}: cannot read duration "${text}"`);
  const scale = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000 }[match[2] ?? 'm'];
  return Math.round(Number(match[1]) * scale);
}

export function formatDuration(ms) {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m${String(seconds % 60).padStart(2, '0')}s`;
  return `${Math.floor(minutes / 60)}h${String(minutes % 60).padStart(2, '0')}m`;
}

export function parseArgs(argv, env = process.env) {
  const options = {
    director: false,
    wait: true,
    maxTime: parseDuration(env.PSF_GUARD_E2E_MAX_TIME ?? '30m', 'PSF_GUARD_E2E_MAX_TIME'),
    idle: parseDuration(env.PSF_GUARD_E2E_IDLE ?? '5m', 'PSF_GUARD_E2E_IDLE'),
    grace: 10_000,
    help: false,
    playwrightArgs: [],
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${arg} needs a value`);
      i += 1;
      return argv[i];
    };
    if (arg === '--') {
      options.playwrightArgs.push(...argv.slice(i + 1));
      break;
    } else if (arg === '--director') options.director = true;
    else if (arg === '--no-wait') options.wait = false;
    else if (arg === '--max-time') options.maxTime = parseDuration(value(), arg);
    else if (arg === '--idle') options.idle = parseDuration(value(), arg);
    else if (arg === '--grace') options.grace = parseDuration(value(), arg);
    else if (arg === '-h' || arg === '--help') options.help = true;
    else options.playwrightArgs.push(arg);
  }
  return options;
}

// ---------------------------------------------------------------- processes

/** Every process on the machine, from POSIX `ps` (macOS and Linux agree). */
export function listProcesses() {
  const text = execFileSync('ps', ['-A', '-o', 'pid=,ppid=,pgid=,stat=,comm='], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  return parsePs(text);
}

export function parsePs(text) {
  const rows = [];
  for (const line of text.split('\n')) {
    const match = /^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/.exec(line);
    if (match) {
      rows.push({
        pid: Number(match[1]),
        ppid: Number(match[2]),
        pgid: Number(match[3]),
        stat: match[4],
        comm: match[5].trim(),
      });
    }
  }
  return rows;
}

// WebKit runs as `Playwright` from ms-playwright's Playwright.app; Chromium as
// `chrome-headless-shell` (Linux truncates comm to `chrome-headless`).
const BROWSER_COMMAND = /ms-playwright|playwright|chrome-headless|headless_shell/i;

/**
 * Browsers macOS has marked as exiting (`E` in the stat column). They ignore
 * every signal, SIGKILL included, and new browser launches hang behind them.
 * Only a reboot clears them.
 */
export function stuckBrowsers(processes) {
  return processes.filter((p) => p.stat.includes('E') && BROWSER_COMMAND.test(p.comm));
}

/** `root` and everything below it, by parent pid. */
export function descendants(processes, root) {
  const children = new Map();
  for (const p of processes) {
    if (!children.has(p.ppid)) children.set(p.ppid, []);
    children.get(p.ppid).push(p.pid);
  }
  const found = new Set([root]);
  const queue = [root];
  while (queue.length) {
    for (const child of children.get(queue.pop()) ?? []) {
      if (!found.has(child)) {
        found.add(child);
        queue.push(child);
      }
    }
  }
  return found;
}

/**
 * Process groups this run owns: the runner's own and every group a
 * descendant created. Playwright starts each browser and web server as a
 * group leader of its own, so the runner's group alone misses them.
 */
export function ownedGroups(processes, root) {
  const tree = descendants(processes, root);
  const groups = new Set([root]);
  for (const p of processes) {
    if (tree.has(p.pid) && (p.pgid === root || tree.has(p.pgid))) groups.add(p.pgid);
  }
  return groups;
}

export function groupMembers(processes, groups) {
  return processes.filter((p) => groups.has(p.pgid));
}

export function isAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

function processStart(pid) {
  try {
    const text = execFileSync('ps', ['-o', 'lstart=', '-p', String(pid)], { encoding: 'utf8' }).trim();
    const time = Date.parse(text);
    return Number.isNaN(time) ? null : time;
  } catch {
    return null;
  }
}

/** Alive, and not a newer process that reused a dead owner's pid. */
export function ownerAlive(owner) {
  if (!Number.isInteger(owner?.pid) || !isAlive(owner.pid)) return false;
  const started = processStart(owner.pid);
  const claimed = Date.parse(owner.started);
  return started === null || Number.isNaN(claimed) || started <= claimed + 5_000;
}

// --------------------------------------------------------------------- lock

const OWNER_FILE = 'owner.json';

export function readOwner(lockDir) {
  try {
    return JSON.parse(fs.readFileSync(path.join(lockDir, OWNER_FILE), 'utf8'));
  } catch {
    return null;
  }
}

/** `mkdir` is atomic: exactly one caller creates the directory. */
export function tryAcquire(lockDir, owner) {
  try {
    fs.mkdirSync(lockDir);
  } catch (error) {
    if (error.code === 'EEXIST') return false;
    throw error;
  }
  const temporary = path.join(lockDir, `${OWNER_FILE}.${process.pid}.tmp`);
  fs.writeFileSync(temporary, `${JSON.stringify(owner)}\n`);
  fs.renameSync(temporary, path.join(lockDir, OWNER_FILE));
  return true;
}

/**
 * Remove a lock whose owner is dead. A second guard directory serialises
 * breakers, and the owner is read again inside it, so two waiters cannot
 * both break the lock and one remove the other's fresh claim.
 *
 * `stale` is the owner the caller saw, or null for a lock with no owner file
 * (its creator died between `mkdir` and writing one).
 */
export function breakStaleLock(lockDir, stale, now = Date.now()) {
  const guard = `${lockDir}.break`;
  try {
    fs.mkdirSync(guard);
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
    try {
      if (now - fs.statSync(guard).mtimeMs > 60_000) fs.rmSync(guard, { recursive: true, force: true });
    } catch {
      // Another breaker finished in the meantime.
    }
    return false;
  }
  try {
    const current = readOwner(lockDir);
    if (stale) {
      if (!current || current.pid !== stale.pid || current.started !== stale.started) return false;
      if (ownerAlive(current)) return false;
    } else {
      if (current) return false;
      let age;
      try {
        age = now - fs.statSync(lockDir).mtimeMs;
      } catch {
        return false;
      }
      if (age < 30_000) return false;
    }
    fs.rmSync(lockDir, { recursive: true, force: true });
    return true;
  } finally {
    fs.rmSync(guard, { recursive: true, force: true });
  }
}

export function releaseLock(lockDir, owner) {
  const current = readOwner(lockDir);
  if (current && current.pid === owner.pid && current.started === owner.started) {
    fs.rmSync(lockDir, { recursive: true, force: true });
  }
}

export function describeOwner(owner, now = Date.now()) {
  if (!owner) return 'a run that is still writing its lock';
  const started = Date.parse(owner.started);
  const when = Number.isNaN(started)
    ? owner.started
    : `${new Date(started).toLocaleString()}, ${formatDuration(now - started)} ago`;
  return `${owner.worktree} (pid ${owner.pid}, started ${when})`;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export class LockBusy extends Error {}

export async function acquireLock(lockDir, owner, { wait, log, pollMs = 2_000 }) {
  let shown = null;
  let shownAt = 0;
  for (;;) {
    if (tryAcquire(lockDir, owner)) return;
    const current = readOwner(lockDir);
    if (current && !ownerAlive(current)) {
      if (breakStaleLock(lockDir, current)) {
        log(`broke a stale lock from ${describeOwner(current)}: pid ${current.pid} is gone`);
        continue;
      }
    } else if (!current && breakStaleLock(lockDir, null)) {
      log('broke a lock with no owner left behind by a run that died starting');
      continue;
    }
    if (!wait) throw new LockBusy(`another e2e run holds ${lockDir}: ${describeOwner(current)}`);
    const key = current ? `${current.pid} ${current.started}` : '';
    if (key !== shown || Date.now() - shownAt > 60_000) {
      log(`waiting for ${describeOwner(current)}`);
      shown = key;
      shownAt = Date.now();
    }
    await sleep(pollMs);
  }
}

// -------------------------------------------------------------------- ports

/** Stable per-worktree ports in 20000–23999, clear of the ephemeral range. */
export function defaultPorts(worktree) {
  const slot = createHash('sha256').update(worktree).digest().readUInt32BE(0) % 1000;
  const base = 20_000 + slot * 4;
  return { main: base, director: base + 3 };
}

function portFree(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => resolve(false));
    server.listen(port, '127.0.0.1', () => server.close(() => resolve(true)));
  });
}

function portHolder(port) {
  try {
    const text = execFileSync('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fpc'], { encoding: 'utf8' });
    const pid = /^p(\d+)/m.exec(text)?.[1];
    const command = /^c(.*)$/m.exec(text)?.[1];
    return pid ? ` by pid ${pid}${command ? ` (${command})` : ''}` : '';
  } catch {
    return '';
  }
}

// ---------------------------------------------------------------------- run

class Run {
  constructor({ command, args, cwd, env, options, log }) {
    Object.assign(this, { command, args, cwd, env, options, log });
    this.groups = new Set();
    this.stopping = null;
    this.hurry = false;
    this.exited = false;
  }

  refreshGroups() {
    if (this.exited) return;
    try {
      for (const group of ownedGroups(listProcesses(), this.child.pid)) this.groups.add(group);
    } catch {
      // ps can fail on a loaded machine; the next tick tries again.
    }
  }

  liveMembers() {
    try {
      return groupMembers(listProcesses(), this.groups);
    } catch {
      return [];
    }
  }

  signalGroups(signal) {
    for (const group of this.groups) {
      try {
        process.kill(-group, signal);
      } catch {
        // The group is already gone.
      }
    }
  }

  /** SIGTERM this run's groups, give them the grace period, then SIGKILL. */
  async terminate() {
    this.refreshGroups();
    this.signalGroups('SIGTERM');
    const deadline = Date.now() + this.options.grace;
    while (Date.now() < deadline && !this.hurry && this.liveMembers().length) await sleep(250);
    if (this.liveMembers().length) {
      this.log('sending SIGKILL to what is left');
      this.signalGroups('SIGKILL');
      const killDeadline = Date.now() + 5_000;
      while (Date.now() < killDeadline && this.liveMembers().length) await sleep(250);
    }
    const left = this.liveMembers();
    if (left.length) {
      this.log(`${left.length} process(es) did not exit:`);
      for (const p of left) this.log(`  ${p.pid} ${p.stat} ${p.comm}`);
      if (left.some((p) => p.stat.includes('E'))) {
        this.log('processes marked E are stuck exiting; reboot before running e2e again');
      }
    }
  }

  stop(reason) {
    if (this.stopping) {
      if (reason === 'interrupted') this.hurry = true;
      return;
    }
    this.stopping = reason;
    const why = {
      timeout: `hit the ${formatDuration(this.options.maxTime)} time limit`,
      watchdog: `no output for ${formatDuration(this.options.idle)}`,
      interrupted: 'interrupted',
    }[reason];
    this.log(`${why}; stopping this run's process groups (Ctrl-C again to skip the grace period)`);
    this.stopped = this.terminate();
  }

  async start() {
    const startedAt = Date.now();
    let lastOutput = startedAt;
    // `detached` calls setsid(): the runner leads a new session and process
    // group, so the terminal's Ctrl-C reaches only this script, which then
    // stops the run in order.
    this.child = spawn(this.command, this.args, {
      cwd: this.cwd,
      env: this.env,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.groups.add(this.child.pid);
    const exited = new Promise((resolve) => {
      this.child.once('error', (error) => resolve({ code: 127, signal: null, error }));
      this.child.once('exit', (code, signal) => resolve({ code, signal }));
    });
    this.child.stdout.on('data', (chunk) => {
      lastOutput = Date.now();
      process.stdout.write(chunk);
    });
    this.child.stderr.on('data', (chunk) => {
      lastOutput = Date.now();
      process.stderr.write(chunk);
    });

    const onSignal = () => this.stop('interrupted');
    const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'];
    for (const signal of signals) process.on(signal, onSignal);
    // A crash in this script must not orphan the run.
    const onExit = () => {
      if (!this.exited) this.signalGroups('SIGKILL');
    };
    process.on('exit', onExit);

    // A group is found through its parent, so look often: one whose parent
    // exits between two looks is no longer traceable to this run.
    const tracker = setInterval(() => this.refreshGroups(), 2_000);
    const ticker = setInterval(() => {
      const now = Date.now();
      if (now - startedAt > this.options.maxTime) this.stop('timeout');
      else if (now - lastOutput > this.options.idle) this.stop('watchdog');
    }, 500);
    this.refreshGroups();

    const result = await exited;
    this.exited = true;
    clearInterval(tracker);
    clearInterval(ticker);

    if (this.stopping) {
      await this.stopped;
    } else {
      // A clean exit can still leave a web server or browser behind if
      // Playwright's own teardown failed. They keep their group ids.
      const left = this.liveMembers();
      if (left.length) {
        this.log(`stopping ${left.length} process(es) the run left behind`);
        await this.terminate();
      }
    }
    for (const signal of signals) process.off(signal, onSignal);
    process.off('exit', onExit);
    this.child.stdout.destroy();
    this.child.stderr.destroy();
    return { ...result, elapsed: Date.now() - startedAt, reason: this.stopping };
  }
}

// --------------------------------------------------------------------- main

function reportStuck(log) {
  let stuck;
  try {
    stuck = stuckBrowsers(listProcesses());
  } catch (error) {
    log(`could not list processes: ${error.message}`);
    return false;
  }
  if (!stuck.length) return false;
  log(`${stuck.length} Playwright browser process(es) are stuck exiting (E in ps):`);
  for (const p of stuck) log(`  ${p.pid} ${p.stat} ${p.comm}`);
  log('They ignore SIGKILL and new browser launches hang behind them.');
  log('Reboot the machine before running e2e again. Do not retry or pkill.');
  return true;
}

export async function main(argv = process.argv.slice(2), env = process.env) {
  const log = (message) => console.error(`${TAG} ${message}`);
  let options;
  try {
    options = parseArgs(argv, env);
  } catch (error) {
    log(error.message);
    console.error(USAGE);
    return EXIT.refused;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  if (process.platform === 'win32') {
    log('this runner needs POSIX process groups; on Windows run npx playwright test directly');
    return EXIT.refused;
  }

  const worktree = fs.realpathSync(SCRIPT_ROOT);
  const staticDir = path.join(worktree, 'static');
  const binary = env.PSF_GUARD_E2E_BINARY ?? path.join(worktree, 'target', 'release', 'psf-guard');
  try {
    if (!fs.statSync(binary).isFile()) throw new Error('not a file');
    fs.accessSync(binary, fs.constants.X_OK);
  } catch {
    log(`no server binary at ${binary}`);
    log('build one with `cargo build --release`, or point PSF_GUARD_E2E_BINARY at one');
    return EXIT.refused;
  }

  // Tests replace Playwright with a fake command; nobody else should.
  const fakeRunner = env.PSF_GUARD_E2E_LOCAL_RUNNER;
  const playwright = path.join(staticDir, 'node_modules', '.bin', 'playwright');
  if (!fakeRunner && !fs.existsSync(playwright)) {
    log(`no Playwright at ${playwright}; run \`npm ci\` in static/ first`);
    return EXIT.refused;
  }

  const ports = defaultPorts(worktree);
  const mainPort = Number(env.PSF_GUARD_E2E_PORT ?? ports.main);
  const directorPort = Number(env.PSF_GUARD_DIRECTOR_E2E_PORT ?? ports.director);
  const runPorts = options.director ? [directorPort] : [mainPort, mainPort + 1, mainPort + 2];

  if (reportStuck(log)) return EXIT.refused;

  const lockDir = env.PSF_GUARD_E2E_LOCK ?? DEFAULT_LOCK;
  const owner = {
    pid: process.pid,
    worktree,
    started: new Date().toISOString(),
    args: argv,
  };
  // Waiting may take a while; Ctrl-C then just leaves.
  const leave = () => process.exit(EXIT.interrupted);
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, leave);
  const release = () => releaseLock(lockDir, owner);
  process.on('exit', release);
  try {
    await acquireLock(lockDir, owner, { wait: options.wait, log });
  } catch (error) {
    if (error instanceof LockBusy) {
      log(error.message);
      return EXIT.busy;
    }
    throw error;
  }

  try {
    // Browsers may have wedged while this run waited.
    if (reportStuck(log)) return EXIT.refused;

    // The fake runner starts no servers.
    for (const port of fakeRunner ? [] : runPorts) {
      if (!(await portFree(port))) {
        log(`port ${port} is already in use${portHolder(port)}`);
        log('a server from an earlier run may still be up; stop that pid, or set PSF_GUARD_E2E_PORT / PSF_GUARD_DIRECTOR_E2E_PORT');
        return EXIT.refused;
      }
    }

    // Let Playwright's own globalTimeout end the run, with its own teardown,
    // a little before this script's hard limit does.
    const globalTimeout = Math.max(options.maxTime - 60_000, Math.floor(options.maxTime * 0.8));
    const childEnv = {
      ...env,
      PSF_GUARD_E2E_BINARY: binary,
      PSF_GUARD_E2E_PORT: String(mainPort),
      PSF_GUARD_DIRECTOR_E2E_PORT: String(directorPort),
      PSF_GUARD_E2E_GLOBAL_TIMEOUT_MS: String(globalTimeout),
      // A server already on the port belongs to someone else; never reuse it.
      PSF_GUARD_E2E_NO_REUSE: '1',
    };
    const configArgs = options.director ? ['--config', 'playwright.director.config.ts'] : [];
    const [command, args] = fakeRunner
      ? ['/bin/sh', ['-c', fakeRunner, 'e2e-local-runner', ...options.playwrightArgs]]
      : [playwright, ['test', ...configArgs, ...options.playwrightArgs]];

    log(
      `${worktree}: ${options.director ? 'Director suite' : 'main suite'} on port${runPorts.length > 1 ? 's' : ''} ` +
        `${runPorts.length > 1 ? `${runPorts[0]}-${runPorts.at(-1)}` : runPorts[0]}, ` +
        `limit ${formatDuration(options.maxTime)}, watchdog ${formatDuration(options.idle)}`,
    );
    const run = new Run({ command, args, cwd: staticDir, env: childEnv, options, log });
    // From here the run handles Ctrl-C by stopping its process groups.
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.off(signal, leave);
    const result = await run.start();
    const elapsed = formatDuration(result.elapsed);
    if (result.reason === 'timeout') {
      log(`timed out after ${elapsed}`);
      return EXIT.stopped;
    }
    if (result.reason === 'watchdog') {
      log(`watchdog stopped the run after ${elapsed}: no output for ${formatDuration(options.idle)}`);
      return EXIT.stopped;
    }
    if (result.reason === 'interrupted') {
      log(`interrupted after ${elapsed}`);
      return EXIT.interrupted;
    }
    if (result.error) {
      log(`could not start ${command}: ${result.error.message}`);
      return EXIT.refused;
    }
    if (result.code === 0) {
      log(`passed in ${elapsed}`);
      return 0;
    }
    log(`failed in ${elapsed} (${result.signal ? `signal ${result.signal}` : `exit ${result.code}`})`);
    return result.code || 1;
  } finally {
    release();
    process.off('exit', release);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(fs.realpathSync(process.argv[1])).href) {
  main().then(
    (code) => process.exit(code),
    (error) => {
      console.error(`${TAG} ${error.stack ?? error}`);
      process.exit(1);
    },
  );
}
