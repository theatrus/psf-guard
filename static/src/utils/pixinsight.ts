import type { PixInsightSettings as Settings } from '../api/types';

/** One line on whether a run could start, and why not. */
export function describePixInsight(settings: Settings): string {
  const { detection, display } = settings;
  if (!detection.install) {
    return detection.problem
      ? `PixInsight not found: ${detection.problem}`
      : `PixInsight not found. Looked in ${detection.checked.join(', ')}.`;
  }
  const version = detection.install.wbpp_version
    ? `WBPP ${detection.install.wbpp_version}`
    : 'WBPP';
  const where = detection.source === 'configured' ? 'as configured' : 'found';
  const screen =
    display.kind === 'own'
      ? ''
      : display.kind === 'xvfb'
        ? ', headless through xvfb-run'
        : ', but the server has no display and no xvfb-run: a run would fail';
  return `${version} ${where} at ${detection.install.binary}${screen}.`;
}


/** Free space as a person reads it. */
export function formatFree(bytes: number): string {
  if (bytes >= 1024 ** 4) return `${(bytes / 1024 ** 4).toFixed(1)} TiB`;
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
  return `${Math.round(bytes / 1024 ** 2)} MiB`;
}
