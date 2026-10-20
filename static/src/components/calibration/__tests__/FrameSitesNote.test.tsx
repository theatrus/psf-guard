import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import type { FrameSitesStatus } from '../../../api/types';
import { server } from '../../../test/msw-server';
import FrameSitesNote from '../FrameSitesNote';

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const status = (state: FrameSitesStatus['state']): FrameSitesStatus => ({
  state, label: 'Reading frame sites · 300 of 1,867 frames', active: false, done: 300, total: 1867,
  waiting_for: null, retry_at: null,
  started_at: 1, finished_at: 2, last: null, last_error: null,
});

describe('FrameSitesNote', () => {
  it('says reading frame sites is stopped and takes it up again on Resume', async () => {
    let resumed = 0;
    server.use(
      http.get('/api/db/demo/calibrations/frame-sites', () =>
        HttpResponse.json({ success: true, data: status(resumed ? 'queued' : 'stopped'), error: null })),
      http.post('/api/db/demo/calibrations/frame-sites/resume', () => {
        resumed += 1;
        return HttpResponse.json({ success: true, data: status('queued'), error: null });
      }),
    );
    render(<FrameSitesNote dbId="demo" />, { wrapper: wrapper() });
    expect(await screen.findByTestId('frame-sites-stopped')).toHaveTextContent('Reading frame sites is stopped');
    fireEvent.click(screen.getByTestId('frame-sites-resume'));
    await waitFor(() => expect(resumed).toBe(1));
    await waitFor(() => expect(screen.queryByTestId('frame-sites-stopped')).toBeNull());
  });

  it('says nothing while the job runs, waits or rests', async () => {
    server.use(
      http.get('/api/db/demo/calibrations/frame-sites', () =>
        HttpResponse.json({ success: true, data: status('running'), error: null })),
    );
    const { container } = render(<FrameSitesNote dbId="demo" />, { wrapper: wrapper() });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(container).toBeEmptyDOMElement();
  });
});
