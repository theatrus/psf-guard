import { readFile, writeFile } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function mergeObjects(base, override) {
  const result = { ...base };
  for (const [key, value] of Object.entries(override ?? {})) {
    result[key] = value && typeof value === 'object' && !Array.isArray(value)
      ? mergeObjects(result[key] ?? {}, value)
      : value;
  }
  return result;
}

export async function writeUpdaterConfig(outputPath, overlayPath) {
  const mainConfig = JSON.parse(await readFile(
    path.join(repositoryRoot, 'tauri.conf.json'),
    'utf8',
  ));
  const overlay = overlayPath
    ? JSON.parse(await readFile(path.resolve(overlayPath), 'utf8'))
    : {};
  const config = mergeObjects(overlay, {
    bundle: { createUpdaterArtifacts: true },
    plugins: { updater: mainConfig.plugins.updater },
  });
  await writeFile(outputPath, `${JSON.stringify(config, null, 2)}\n`);
}

// Node gives import.meta.url with links resolved and argv[1] as typed, so
// compare real paths: through a symlink or junction (our Windows runners'
// _work) the two differ, and the script would exit 0 having done nothing.
if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const [outputPath, overlayPath] = process.argv.slice(2);
  if (!outputPath) {
    throw new Error('Usage: node scripts/write-updater-config.mjs OUTPUT [OVERLAY]');
  }
  await writeUpdaterConfig(outputPath, overlayPath);
}
