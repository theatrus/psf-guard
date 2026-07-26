import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import UpdateNotice from '../UpdateNotice';
import { GITHUB_NOTICE_URL, WEBSITE_NOTICE_URL } from '../../updates/releases';

afterEach(() => vi.unstubAllGlobals());

it('shows and dismisses a server update from the website-first notice flow', async () => {
  const response = {
    schema_version: 1,
    version: '0.6.0',
    release_url: 'https://github.com/theatrus/psf-guard/releases/tag/v0.6.0',
    summary: 'Improves catalog review.',
    urgency: 'normal',
    minimum_supported_version: '0.5.0',
    published_at: '2026-07-26T18:00:00Z',
  };
  const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(response), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  }));
  vi.stubGlobal('fetch', fetchMock);

  const user = userEvent.setup();
  render(<UpdateNotice installedVersion="0.5.0" />);

  expect(await screen.findByText('v0.6.0 available')).toBeInTheDocument();
  expect(screen.getByText('Improves catalog review.')).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Release notes' })).toHaveAttribute(
    'href',
    response.release_url,
  );
  expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
    WEBSITE_NOTICE_URL,
    GITHUB_NOTICE_URL,
  ]);

  await user.click(screen.getByRole('button', { name: 'Dismiss v0.6.0 update notice' }));
  expect(screen.queryByText('v0.6.0 available')).not.toBeInTheDocument();
});
