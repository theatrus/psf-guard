import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const studio = fileURLToPath(new URL('../studio/', import.meta.url));
const targetDir = path.resolve(process.env.STUDIO_CARGO_TARGET_DIR || path.join(studio, 'target'));
const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET;
const args = ['build', '--locked', '--release', '-p', 'psf-studio', '--target-dir', targetDir];
if (target) args.push('--target', target);

// Studio selects its own pinned toolchain from studio/rust-toolchain.toml.
const env = { ...process.env };
delete env.RUSTUP_TOOLCHAIN;
const build = spawnSync('cargo', args, { cwd: studio, env, stdio: 'inherit' });
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);

const executable = path.join(targetDir, target ?? '', 'release', 'psf-studio.exe');
const staging = path.join(studio, 'target', 'windows-bundle');
mkdirSync(staging, { recursive: true });
copyFileSync(executable, path.join(staging, 'psf-studio.exe'));
