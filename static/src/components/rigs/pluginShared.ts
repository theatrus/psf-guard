import type { SyncContactSummary } from '../../api/types';

/** This server's address as a plugin needs it; a desktop window has none to offer. */
export function serverAddress(): string | null {
  const { protocol, origin, pathname } = window.location;
  return /^https?:$/.test(protocol) ? `${origin}${pathname.replace(/\/$/, '')}` : null;
}

/** An address a plugin reaches over plain HTTP from another machine. */
export const needsHttps = (address: string | null) => !!address && address.startsWith('http:')
  && !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/.test(address);

/** "5 min ago", "just now", or the date and time. */
export function ago(ms: number): string {
  const minutes = Math.round((Date.now() - ms) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  return new Date(ms).toLocaleString();
}

/** When a Sync client last called, and which plugin version it reported. */
export function contactText(contact: SyncContactSummary | undefined): string {
  if (!contact) return 'no contact recorded';
  return `last contact ${ago(contact.at * 1000)}${contact.version ? `, plugin ${contact.version}` : ''}`;
}

export const DEFAULT_UPLOAD_LAYOUT = '%YEAR%/%TARGET%/%NIGHT%/%TYPE%';
export const UPLOAD_LAYOUT_PRESETS = [
  '%YEAR%/%TARGET%/%NIGHT%/%TYPE%',
  '%TARGET%/%NIGHT%/%TYPE%',
  '%TARGET%/%DATE%/%TYPE%',
  '%TARGET%/%TYPE%/%FILTER%',
  '%NIGHT%/%TARGET%/%TYPE%',
] as const;
export const isUploadLayoutPreset = (value: string) => UPLOAD_LAYOUT_PRESETS.some(preset => preset === value);
