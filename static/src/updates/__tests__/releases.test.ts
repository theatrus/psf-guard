import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  fetchAvailableUpdate,
  GITHUB_NOTICE_URL,
  parseReleaseNotice,
  selectNewestUpdate,
  WEBSITE_NOTICE_URL,
} from '../releases';

const notice = (version: string) => ({
  schema_version: 1,
  version,
  release_url: `https://github.com/theatrus/psf-guard/releases/tag/v${version}`,
  summary: 'Improves catalog review.',
  urgency: 'recommended',
  minimum_supported_version: '0.4.0',
  published_at: '2026-07-26T18:00:00Z',
});

afterEach(() => vi.unstubAllGlobals());

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

  it('reads the website before GitHub and keeps the newer valid feed', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(notice('0.6.0')), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }))
      .mockResolvedValueOnce(new Response(JSON.stringify(notice('0.7.0')), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(fetchAvailableUpdate('0.5.0', new AbortController().signal))
      .resolves.toMatchObject({ version: '0.7.0' });
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      WEBSITE_NOTICE_URL,
      GITHUB_NOTICE_URL,
    ]);
  });

  it('falls back to the GitHub feed after the website feed fails', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(null, { status: 503 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(notice('0.6.0')), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(fetchAvailableUpdate('0.5.0', new AbortController().signal))
      .resolves.toMatchObject({ version: '0.6.0' });
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      WEBSITE_NOTICE_URL,
      GITHUB_NOTICE_URL,
    ]);
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
