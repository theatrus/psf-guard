import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import CalibratedCopiesControls from '../CalibratedCopiesControls';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });

function mount(canManage: boolean, counts = { calibrated: 341, registered: 666, calibrated_lights: 2 }) {
  const saved = vi.fn(async ({ request }: { request: Request }) => {
    const body = (await request.json()) as { pair: boolean };
    return ok({ pair: body.pair, counts });
  });
  server.use(
    http.get('/api/db/c925/calibrated-copies', () => ok({ pair: true, counts })),
    http.put('/api/db/c925/calibrated-copies', saved),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...render(<CalibratedCopiesControls dbId="c925" canManage={canManage} />, { wrapper: Wrapper }), saved };
}

describe('calibrated copies setting', () => {
  it('shows what is paired and turns pairing off', async () => {
    const { saved } = mount(true);
    const toggle = await screen.findByRole('checkbox', { name: 'Pair calibrated and registered copies with their lights' });
    expect(toggle).toBeChecked();
    expect(screen.getByText(/341 calibrated and 666 registered copies paired/)).toBeInTheDocument();
    expect(screen.getByText(/2 lights come from a calibrated copy/)).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(await screen.findByText(/Off: copies import as lights of their own/)).toBeInTheDocument();
    expect(saved).toHaveBeenCalledTimes(1);
    expect(toggle).not.toBeChecked();
  });

  it('is read-only without database management', async () => {
    mount(false, { calibrated: 0, registered: 0, calibrated_lights: 0 });
    const toggle = await screen.findByRole('checkbox');
    expect(toggle).toBeDisabled();
    expect(screen.getByText(/No copies paired yet/)).toBeInTheDocument();
    expect(screen.getByText(/needs database management/)).toBeInTheDocument();
  });
});
