import type { ReleaseNotice } from '../api/types';
import { isVersionNewer } from './version';

export const RELEASES_URL = 'https://github.com/theatrus/psf-guard/releases/latest';

export type UpdateUrgency = 'normal' | 'recommended' | 'required';

export type AvailableUpdate = {
  version: string;
  url: string;
  summary?: string;
  urgency: UpdateUrgency;
  source: 'notice' | 'signed';
};

function normalizedVersion(version: string): string {
  return version.trim().replace(/^v/, '');
}

function safeReleaseUrl(value: unknown): value is string {
  if (typeof value !== 'string') return false;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && (
      url.hostname === 'updates.psf-guard.com' ||
      (url.hostname === 'github.com' &&
        url.pathname.startsWith('/theatrus/psf-guard/releases/'))
    );
  } catch {
    return false;
  }
}

export function parseReleaseNotice(
  value: unknown,
  currentVersion: string,
): AvailableUpdate | null {
  const notice = value as ReleaseNotice;
  if (
    !notice ||
    notice.schema_version !== 1 ||
    typeof notice.version !== 'string' ||
    !safeReleaseUrl(notice.release_url) ||
    !isVersionNewer(notice.version, currentVersion)
  ) {
    return null;
  }

  const configuredUrgency: UpdateUrgency =
    notice.urgency === 'recommended' || notice.urgency === 'required'
      ? notice.urgency
      : 'normal';
  const minimumRequiresUpdate =
    typeof notice.minimum_supported_version === 'string' &&
    isVersionNewer(notice.minimum_supported_version, currentVersion);
  const summary = typeof notice.summary === 'string'
    ? notice.summary.trim().slice(0, 240)
    : undefined;

  return {
    version: normalizedVersion(notice.version),
    url: notice.release_url,
    summary: summary || undefined,
    urgency: minimumRequiresUpdate ? 'required' : configuredUrgency,
    source: 'notice',
  };
}

export function signedUpdate(version: string): AvailableUpdate {
  return {
    version: normalizedVersion(version),
    url: RELEASES_URL,
    urgency: 'normal',
    source: 'signed',
  };
}

export function selectNewestUpdate(
  notice: AvailableUpdate | null,
  signedVersion: string | null,
  currentVersion: string,
): AvailableUpdate | null {
  if (!signedVersion || !isVersionNewer(signedVersion, currentVersion)) return notice;
  if (!notice || isVersionNewer(signedVersion, notice.version)) {
    return signedUpdate(signedVersion);
  }
  return notice;
}
