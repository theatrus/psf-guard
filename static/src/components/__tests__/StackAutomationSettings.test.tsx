import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import StackAutomationSettings from '../StackAutomationSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const current = (enabled: boolean, arrival = 5, grade = 15) => ({
  success: true,
  data: {
    automatic_previews: enabled,
    arrival_delay_minutes: arrival,
    grade_delay_minutes: grade,
    default_arrival_delay_minutes: 5,
    default_grade_delay_minutes: 15,
    max_delay_minutes: 1440,
  },
  error: null,
});

describe('StackAutomationSettings', () => {
  it('starts off with the delays shown but not editable', async () => {
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(current(false))));
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', {
      name: /Rebuild stack previews on their own/,
    });
    expect(toggle).not.toBeChecked();
    const arrival = screen.getByLabelText('Minutes to wait after new frames');
    expect(arrival).toHaveValue(5);
    expect(arrival).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('turns automation on with its own delays and reflects the server response', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(current(false))),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(true, 5, 30));
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    fireEvent.click(
      await screen.findByRole('checkbox', { name: /Rebuild stack previews on their own/ })
    );
    fireEvent.change(screen.getByLabelText('Minutes to wait after grade changes'), {
      target: { value: '30' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(saved).toEqual({
        automatic_previews: true,
        arrival_delay_minutes: 5,
        grade_delay_minutes: 30,
      })
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    );
    expect(screen.getByLabelText('Minutes to wait after grade changes')).toHaveValue(30);
  });

  it('refuses a delay outside the allowed range', async () => {
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(current(true))));
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const arrival = await screen.findByLabelText('Minutes to wait after new frames');
    fireEvent.change(arrival, { target: { value: '0' } });
    expect(screen.getByText(/Enter whole minutes between 1 and 1440/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });
});
