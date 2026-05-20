import * as crypto from 'crypto';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export interface FixtureFile {
  name: string;
  sha256: string;
  size_mb?: number;
}

export interface Manifest {
  release_tag: string;
  base_url: string;
  files: FixtureFile[];
}

/**
 * Local cache directory where downloaded FITS fixtures live between runs.
 * Override with `$PSF_GUARD_E2E_FIXTURE_CACHE`. Default is XDG-style under
 * the user's home so they persist outside the repo and aren't recreated
 * on every test invocation (each file is ~117 MB).
 */
export function fixtureCacheDir(): string {
  return (
    process.env.PSF_GUARD_E2E_FIXTURE_CACHE ??
    path.join(os.homedir(), '.cache', 'psf-guard-e2e-fixtures')
  );
}

export function readManifest(): Manifest {
  const raw = fs.readFileSync(path.join(__dirname, 'manifest.json'), 'utf8');
  return JSON.parse(raw) as Manifest;
}

function sha256(buf: Buffer): string {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

async function downloadFile(url: string, dest: string): Promise<void> {
  // Use the global fetch (Node 20+) — keeps the dep footprint minimal.
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) {
    throw new Error(`GET ${url} failed: ${res.status} ${res.statusText}`);
  }
  const buf = Buffer.from(await res.arrayBuffer());
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  // Write to a temp file then rename so a partial download can't be picked
  // up as cached on a subsequent run.
  const tmp = `${dest}.partial-${process.pid}`;
  fs.writeFileSync(tmp, buf);
  fs.renameSync(tmp, dest);
}

/**
 * Resolve a fixture path: return the cached file if its sha256 matches the
 * manifest, otherwise download it from the manifest's base_url and verify.
 * Throws if the download succeeds but the checksum is wrong.
 */
export async function ensureFixture(file: FixtureFile, manifest: Manifest): Promise<string> {
  const cachedPath = path.join(fixtureCacheDir(), file.name);
  const expected = file.sha256.toLowerCase();

  if (fs.existsSync(cachedPath)) {
    const actual = sha256(fs.readFileSync(cachedPath));
    if (actual === expected) return cachedPath;
    console.warn(
      `[psf-guard e2e] Cached ${file.name} has unexpected sha256 ` +
        `(${actual}); redownloading.`
    );
    fs.rmSync(cachedPath, { force: true });
  }

  const base = process.env.PSF_GUARD_E2E_FIXTURE_BASE ?? manifest.base_url;
  const url = `${base.replace(/\/$/, '')}/${encodeURIComponent(file.name)}`;
  console.log(`[psf-guard e2e] Downloading ${file.name} from ${url}`);
  await downloadFile(url, cachedPath);

  const actual = sha256(fs.readFileSync(cachedPath));
  if (actual !== expected) {
    fs.rmSync(cachedPath, { force: true });
    throw new Error(
      `Downloaded ${file.name} has sha256 ${actual}, expected ${expected}`
    );
  }
  return cachedPath;
}

/** Resolve all fixtures listed in the manifest, returning a name → path map. */
export async function ensureAllFixtures(): Promise<Record<string, string>> {
  const manifest = readManifest();
  const out: Record<string, string> = {};
  for (const f of manifest.files) {
    out[f.name] = await ensureFixture(f, manifest);
  }
  return out;
}
