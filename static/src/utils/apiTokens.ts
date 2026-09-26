import type { AuthTokenSummary } from '../api/types';

export function formatTokenDate(seconds: number): string {
  return new Date(seconds * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

export function describeTokenExpiry(token: AuthTokenSummary, now = Date.now()): string {
  if (token.expires_at === undefined) return 'Never expires';
  if (token.expires_at * 1000 <= now) return 'Expired';
  return `Expires ${formatTokenDate(token.expires_at)}`;
}
