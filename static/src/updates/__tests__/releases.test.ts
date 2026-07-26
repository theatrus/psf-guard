import { describe, expect, it } from 'vitest';
import { parseReleaseNotice, selectNewestUpdate } from '../releases';

const notice = (version: string) => ({
  schema_version: 1,
  version,
  release_url: `https://github.com/theatrus/psf-guard/releases/tag/v${version}`,
  summary: 'Improves catalog review.',
  urgency: 'recommended' as const,
  minimum_supported_version: '0.4.0',
  published_at: '2026-07-26T18:00:00Z',
});

describe('release notices', () => {
  it('accepts a newer valid notice and rejects unsafe links', () => {
    expect(parseReleaseNotice(notice('0.6.0'), '0.5.0')).toMatchObject({
      version: '0.6.0',
      urgency: 'recommended',
      source: 'notice',
    });
    expect(parseReleaseNotice({
      ...notice('0.6.0'),
      release_url: 'https://example.com/download',
    }, '0.5.0')).toBeNull();
  });

  it('keeps notice copy when a signed package offers the same version', () => {
    const parsed = parseReleaseNotice(notice('0.6.0'), '0.5.0');
    expect(selectNewestUpdate(parsed, '0.6.0', '0.5.0')).toBe(parsed);
    expect(selectNewestUpdate(parsed, '0.7.0', '0.5.0')).toMatchObject({
      version: '0.7.0',
      source: 'signed',
    });
  });
});
