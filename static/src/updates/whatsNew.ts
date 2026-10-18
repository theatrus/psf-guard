import bundledHighlights from './highlights.json';
import { isVersionNewer, parseVersion } from './version';

export interface ReleaseHighlight {
  title: string;
  body: string;
}

export interface ReleaseHighlights {
  version: string;
  deprecations: ReleaseHighlight[];
  upgradeNotes: ReleaseHighlight[];
  highlights: ReleaseHighlight[];
}

export const WHATS_NEW_KEY = 'psf-guard.whats-new.acknowledged';
export const releaseHighlights: ReleaseHighlights[] = bundledHighlights;

export function normalizedVersion(value?: string): string | null {
  return value && parseVersion(value) ? value.trim().replace(/^v/, '').split('+')[0] : null;
}

export function readAcknowledgedVersion(): string | null {
  try {
    return normalizedVersion(localStorage.getItem(WHATS_NEW_KEY) ?? undefined);
  } catch {
    return null;
  }
}

export function acknowledgeVersion(version: string): void {
  const previous = readAcknowledgedVersion();
  // A second tab or a downgraded server must not undo a newer acknowledgment.
  if (previous && !isVersionNewer(version, previous)) return;
  try {
    localStorage.setItem(WHATS_NEW_KEY, version);
  } catch {
    // The dialog still closes when the browser blocks storage.
  }
}

export function notesForVersion(
  version: string | null,
  after: string | null = null,
  releases: ReleaseHighlights[] = releaseHighlights,
): ReleaseHighlights[] {
  if (!version) return [];
  return releases.filter(release =>
    !isVersionNewer(release.version, version)
    && (!after || isVersionNewer(release.version, after)),
  ).sort((a, b) => isVersionNewer(a.version, b.version) ? -1 : 1);
}

export function releaseNotesUrl(version: string): string {
  return `https://github.com/theatrus/psf-guard/releases/tag/v${encodeURIComponent(version)}`;
}
