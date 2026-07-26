import { isVersionNewer } from './version';

export const WEBSITE_NOTICE_URL = 'https://updates.psf-guard.com/notice.json';
export const GITHUB_NOTICE_URL =
  'https://github.com/theatrus/psf-guard/releases/latest/download/notice.json';
export const RELEASES_URL = 'https://github.com/theatrus/psf-guard/releases/latest';

export type UpdateUrgency = 'normal' | 'recommended' | 'required';

export type AvailableUpdate = {
  version: string;
  url: string;
  summary?: string;
  urgency: UpdateUrgency;
  source: 'notice' | 'signed';
};

type ReleaseNotice = {
  schema_version?: unknown;
  version?: unknown;
  release_url?: unknown;
  summary?: unknown;
  urgency?: unknown;
  minimum_supported_version?: unknown;
  published_at?: unknown;
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

async function fetchJson(url: string, signal: AbortSignal): Promise<unknown | null> {
  const response = await fetch(url, {
    cache: 'no-store',
    headers: { Accept: 'application/json' },
    signal,
  });
  if (!response.ok) return null;
  return response.json() as Promise<unknown>;
}

/** Read the website first, then GitHub. Keep the newer valid notice; the
 * website copy wins when both describe the same version. */
export async function fetchAvailableUpdate(
  currentVersion: string,
  signal: AbortSignal,
): Promise<AvailableUpdate | null> {
  let selected: AvailableUpdate | null = null;
  for (const url of [WEBSITE_NOTICE_URL, GITHUB_NOTICE_URL]) {
    try {
      const notice = parseReleaseNotice(await fetchJson(url, signal), currentVersion);
      if (notice && (!selected || isVersionNewer(notice.version, selected.version))) {
        selected = notice;
      }
    } catch (error) {
      if (signal.aborted) throw error;
    }
  }
  return selected;
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
