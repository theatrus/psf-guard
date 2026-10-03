import { beforeEach, describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import QualityBackfillControls from '../QualityBackfillControls';
import { setStarMetadataFill } from '../../hooks/useStarMetadataFill';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const idleStatus = {
  success: true,
  data: {
    started: false,
    progress: {
      running: false,
      force: false,
      total_targets: 0,
      processed_targets: 0,
      current_target_id: null,
      started_at: null,
      finished_at: null,
    },
  },
  error: null,
  status: 'ready',
};

describe('QualityBackfillControls star-metadata option', () => {
  beforeEach(() => {
    setStarMetadataFill(true);
    server.use(
      http.get('/api/db/:dbId/analysis/quality-backfill', () =>
        HttpResponse.json(idleStatus)
      )
    );
  });

  it('sends the checkbox state as fill_metadata when starting analysis', async () => {
    const bodies: unknown[] = [];
    server.use(
      http.post('/api/db/:dbId/analysis/quality-backfill', async ({ request }) => {
        bodies.push(await request.json());
        return HttpResponse.json(idleStatus);
      })
    );

    const user = userEvent.setup();
    render(<QualityBackfillControls dbId="test" />, { wrapper: wrapper() });

    const checkbox = await screen.findByRole('checkbox', {
      name: /write star count and hfr/i,
    });
    expect(checkbox).toBeChecked();
    expect(
      screen.getByText(/your fits and xisf files are never modified/i)
    ).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Analyze Missing Quality' }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toMatchObject({ force: false, fill_metadata: true });

    await user.click(checkbox);
    expect(checkbox).not.toBeChecked();

    await user.click(screen.getByRole('button', { name: 'Analyze Missing Quality' }));
    await waitFor(() => expect(bodies).toHaveLength(2));
    expect(bodies[1]).toMatchObject({ force: false, fill_metadata: false });
  });

  it('remembers the opt-out as the default for a fresh mount', async () => {
    const user = userEvent.setup();
    const first = render(<QualityBackfillControls dbId="test" />, { wrapper: wrapper() });
    await user.click(
      await screen.findByRole('checkbox', { name: /write star count and hfr/i })
    );
    first.unmount();

    render(<QualityBackfillControls dbId="test" />, { wrapper: wrapper() });
    expect(
      await screen.findByRole('checkbox', { name: /write star count and hfr/i })
    ).not.toBeChecked();
  });
});

describe('QualityBackfillControls new-frame analysis', () => {
  it('saves the choice to analyze frames as they arrive', async () => {
    let enabled = false;
    const status = () => ({ ...idleStatus, data: { ...idleStatus.data, analyze_new_frames: enabled } });
    const bodies: unknown[] = [];
    server.use(
      http.get('/api/db/:dbId/analysis/quality-backfill', () => HttpResponse.json(status())),
      http.put('/api/db/:dbId/analysis/quality-backfill/new-frames', async ({ request }) => {
        const body = (await request.json()) as { enabled: boolean };
        bodies.push(body);
        enabled = body.enabled;
        return HttpResponse.json(status());
      })
    );

    const user = userEvent.setup();
    render(<QualityBackfillControls dbId="test" />, { wrapper: wrapper() });

    const checkbox = await screen.findByRole('checkbox', { name: /analyze new frames as they arrive/i });
    await waitFor(() => expect(checkbox).toBeEnabled());
    expect(checkbox).not.toBeChecked();

    await user.click(checkbox);

    await waitFor(() => expect(bodies).toEqual([{ enabled: true }]));
    await waitFor(() => expect(checkbox).toBeChecked());
  });
});
