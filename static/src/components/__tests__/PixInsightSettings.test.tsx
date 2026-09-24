import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import PixInsightSettings from '../PixInsightSettings';
import { describePixInsight } from '../../utils/pixinsight';
import type { PixInsightSettings as Settings } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const found: Settings = {
  binary: null,
  detection: {
    install: {
      binary: '/opt/PixInsight/bin/PixInsight.sh',
      root: '/opt/PixInsight',
      bpp_main: '/opt/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js',
      wbpp_version: '3.1.0',
    },
    source: 'detected',
    checked: ['/opt/PixInsight/bin/PixInsight.sh'],
    problem: null,
  },
  display: { kind: 'xvfb', path: '/usr/bin/xvfb-run' },
  ready: true,
};

const missing: Settings = {
  binary: '/nope/PixInsight.sh',
  detection: { install: null, source: null, checked: ['/nope/PixInsight.sh'], problem: '/nope/PixInsight.sh is not a file' },
  display: { kind: 'own' },
  ready: false,
};

const ok = (data: Settings) => ({ success: true, data, error: null });

describe('PixInsightSettings', () => {
  it('describes what was found and how it will get a display', () => {
    expect(describePixInsight(found)).toBe(
      'WBPP 3.1.0 found at /opt/PixInsight/bin/PixInsight.sh, headless through xvfb-run.'
    );
    expect(describePixInsight(missing)).toBe('PixInsight not found: /nope/PixInsight.sh is not a file');
    expect(describePixInsight({ ...found, display: { kind: 'missing' }, ready: false })).toContain(
      'no display and no xvfb-run'
    );
  });

  it('saves an executable path and shows the server\u2019s answer', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/pixinsight', () => HttpResponse.json(ok(missing))),
      http.put('/api/settings/pixinsight', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(
          ok({ ...found, binary: '/home/me/PixInsight/bin/PixInsight.sh', detection: { ...found.detection, source: 'configured' } })
        );
      })
    );
    render(<PixInsightSettings />, { wrapper: wrapper() });
    const status = await screen.findByRole('status');
    expect(status).toHaveTextContent('PixInsight not found');
    const input = screen.getByLabelText('PixInsight executable');
    expect(input).toHaveValue('/nope/PixInsight.sh');
    fireEvent.change(input, { target: { value: '/home/me/PixInsight/bin/PixInsight.sh' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(saved).toEqual({ binary: '/home/me/PixInsight/bin/PixInsight.sh' }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('WBPP 3.1.0 as configured'));
  });
});
