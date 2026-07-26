import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { expect, it } from 'vitest';
import UpdateNotice from '../UpdateNotice';
import { server } from '../../test/msw-server';

it('shows and dismisses a notice returned by the server cache', async () => {
  const response = {
    schema_version: 1,
    version: '0.6.0',
    release_url: 'https://github.com/theatrus/psf-guard/releases/tag/v0.6.0',
    summary: 'Improves catalog review.',
    urgency: 'normal',
    minimum_supported_version: '0.5.0',
    published_at: '2026-07-26T18:00:00Z',
  };
  server.use(http.get('/api/update-notice', () => HttpResponse.json({
    success: true,
    data: { notice: response, checking: false, checked_at_unix_seconds: 1_774_806_400 },
    error: null,
    status: 'ready',
  })));

  const user = userEvent.setup();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <UpdateNotice installedVersion="0.5.0" />
    </QueryClientProvider>,
  );

  expect(await screen.findByText('v0.6.0 available')).toBeInTheDocument();
  expect(screen.getByText('Improves catalog review.')).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Release notes' })).toHaveAttribute(
    'href',
    response.release_url,
  );
  await user.click(screen.getByRole('button', { name: 'Dismiss v0.6.0 update notice' }));
  expect(screen.queryByText('v0.6.0 available')).not.toBeInTheDocument();
});
