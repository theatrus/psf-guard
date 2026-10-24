import { Profiler, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import StackAutomationSettings from '../StackAutomationSettings';

function testClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
}

function wrapper(queryClient = testClient()) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const current = (enabled: boolean, arrival = 5, grade = 15, buildNew = false, windowDays = 7) => ({
  success: true,
  data: {
    automatic_previews: enabled,
    arrival_delay_minutes: arrival,
    grade_delay_minutes: grade,
    default_arrival_delay_minutes: 5,
    default_grade_delay_minutes: 15,
    max_delay_minutes: 1440,
    build_new_channels: buildNew,
    new_channel_window_days: windowDays,
    default_new_channel_window_days: 7,
  },
  error: null,
});

describe('StackAutomationSettings', () => {
  it('starts off with the delays shown but not editable, and no save button', async () => {
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(current(false))));
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', {
      name: /Automatically refresh stack previews/,
    });
    expect(toggle).not.toBeChecked();
    const arrival = screen.getByLabelText('Minutes to wait after new frames');
    expect(arrival).toHaveValue(5);
    expect(arrival).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });

  it('saves the switch as soon as it is flipped', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(current(false))),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(true));
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    fireEvent.click(
      await screen.findByRole('checkbox', { name: /Automatically refresh stack previews/ })
    );
    await waitFor(() =>
      expect(saved).toEqual({
        automatic_previews: true,
        arrival_delay_minutes: 5,
        grade_delay_minutes: 15,
        build_new_channels: false,
        new_channel_window_days: 7,
        reapply_processing: true,
      })
    );
    // The response is the new truth: the delays become editable.
    await waitFor(() =>
      expect(screen.getByLabelText('Minutes to wait after new frames')).toBeEnabled()
    );
  });

  it('commits a delay when its field is left, and snaps an invalid one back', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(current(true))),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(true, 5, 30));
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const grade = await screen.findByLabelText('Minutes to wait after grade changes');
    fireEvent.change(grade, { target: { value: '30' } });
    expect(saved).toBeNull();
    fireEvent.blur(grade);
    await waitFor(() =>
      expect(saved).toEqual({
        automatic_previews: true,
        arrival_delay_minutes: 5,
        grade_delay_minutes: 30,
        build_new_channels: false,
        new_channel_window_days: 7,
        reapply_processing: true,
      })
    );
    await waitFor(() => expect(grade).toHaveValue(30));

    const arrival = screen.getByLabelText('Minutes to wait after new frames');
    fireEvent.change(arrival, { target: { value: '0' } });
    expect(screen.getByText(/Enter whole minutes between 1 and 1440/)).toBeInTheDocument();
    fireEvent.blur(arrival);
    expect(arrival).toHaveValue(5);
    expect(screen.queryByText(/Enter whole minutes/)).toBeNull();
  });

  it('commits a delay on Enter', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(current(true))),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(true, 10, 15));
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const arrival = await screen.findByLabelText('Minutes to wait after new frames');
    fireEvent.change(arrival, { target: { value: '10' } });
    fireEvent.keyDown(arrival, { key: 'Enter' });
    await waitFor(() =>
      expect(saved).toEqual({
        automatic_previews: true,
        arrival_delay_minutes: 10,
        grade_delay_minutes: 15,
        build_new_channels: false,
        new_channel_window_days: 7,
        reapply_processing: true,
      })
    );
  });

  it('saves the new-channel option, which waits on the switch', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(current(true))),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(current(true, 5, 15, true));
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const option = await screen.findByRole('checkbox', { name: /Also stack channels that have no stack yet/ });
    expect(option).toBeEnabled();
    fireEvent.click(option);
    await waitFor(() =>
      expect(saved).toEqual({
        automatic_previews: true,
        arrival_delay_minutes: 5,
        grade_delay_minutes: 15,
        build_new_channels: true,
        new_channel_window_days: 7,
        reapply_processing: true,
      })
    );
    await waitFor(() => expect(option).toBeChecked());
  });

  it('sets how far back a new channel may reach, or any age', async () => {
    const saved: unknown[] = [];
    let state = current(true, 5, 15, true, 7);
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(state)),
      http.put('/api/settings/stacking', async ({ request }) => {
        const body = (await request.json()) as { new_channel_window_days: number };
        saved.push(body);
        state = current(true, 5, 15, true, body.new_channel_window_days);
        return HttpResponse.json(state);
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const days = await screen.findByLabelText("Recent-frame requirement (days)");
    expect(days).toHaveValue(7);
    fireEvent.change(days, { target: { value: '30' } });
    fireEvent.blur(days);
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0]).toMatchObject({ new_channel_window_days: 30 });

    const anyAge = screen.getByRole('checkbox', { name: /Also stack old channels, of any age/ });
    fireEvent.click(anyAge);
    await waitFor(() => expect(saved).toHaveLength(2));
    expect(saved[1]).toMatchObject({ new_channel_window_days: 0 });
    await waitFor(() => expect(anyAge).toBeChecked());
    // The days are set aside, not lost: unticking brings 30 back.
    expect(days).toBeDisabled();
    expect(days).toHaveValue(30);
    fireEvent.click(anyAge);
    await waitFor(() => expect(saved).toHaveLength(3));
    expect(saved[2]).toMatchObject({ new_channel_window_days: 30 });
  });

  it('re-applies processing to refreshed stacks unless switched off, even with refreshes off', async () => {
    let saved: Record<string, unknown> | null = null;
    let reapply = true;
    const answer = () =>
      HttpResponse.json({ ...current(false), data: { ...current(false).data, reapply_processing: reapply } });
    server.use(
      http.get('/api/settings/stacking', answer),
      http.put('/api/settings/stacking', async ({ request }) => {
        saved = (await request.json()) as Record<string, unknown>;
        reapply = saved.reapply_processing as boolean;
        return answer();
      })
    );
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    const toggle = await screen.findByRole('checkbox', {
      name: /Re-apply processing when a stack refreshes/,
    });
    // A person's rebuild refreshes a stack too, so it does not wait on the
    // automatic refresh switch.
    expect(toggle).toBeChecked();
    expect(toggle).toBeEnabled();
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(saved).toMatchObject({ automatic_previews: false, reapply_processing: false })
    );
    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  it('shows re-applying on for a server that has no such setting yet', async () => {
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(current(true))));
    render(<StackAutomationSettings />, { wrapper: wrapper() });
    expect(
      await screen.findByRole('checkbox', { name: /Re-apply processing when a stack refreshes/ })
    ).toBeChecked();
  });

  it('shows the saved delays in the first render with settings', () => {
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(current(true, 5, 15, true, 7))));
    const queryClient = testClient();
    queryClient.setQueryData(['stack-automation-settings'], current(true, 5, 15, true, 7).data);
    // Read the DOM at every commit: a render that shows the settings with an
    // empty field is what a test or a person can catch between two commits.
    const shown: string[] = [];
    const record = () => {
      const fields = ['Minutes to wait after new frames', 'Minutes to wait after grade changes', 'Recent-frame requirement (days)']
        .map((label) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)?.value);
      if (fields.every((value) => value !== undefined)) shown.push(fields.join(' '));
    };
    render(
      <Profiler id="stack-automation-settings" onRender={record}>
        <StackAutomationSettings />
      </Profiler>,
      { wrapper: wrapper(queryClient) }
    );
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.filter((values) => values !== '5 15 7')).toEqual([]);
  });

  it('a refetch updates an unedited delay and keeps one being edited', async () => {
    let answer = current(true, 5, 15);
    server.use(http.get('/api/settings/stacking', () => HttpResponse.json(answer)));
    const queryClient = testClient();
    render(<StackAutomationSettings />, { wrapper: wrapper(queryClient) });
    const grade = await screen.findByLabelText('Minutes to wait after grade changes');
    fireEvent.change(grade, { target: { value: '45' } });
    answer = current(true, 10, 20);
    await act(() => queryClient.refetchQueries({ queryKey: ['stack-automation-settings'] }));
    await waitFor(() => expect(screen.getByLabelText('Minutes to wait after new frames')).toHaveValue(10));
    expect(grade).toHaveValue(45);
  });

  it('shows the saved days when another client clears any age', async () => {
    let answer = current(true, 5, 15, true, 30);
    server.use(
      http.get('/api/settings/stacking', () => HttpResponse.json(answer)),
      http.put('/api/settings/stacking', () => {
        answer = current(true, 5, 15, true, 0);
        return HttpResponse.json(answer);
      })
    );
    const queryClient = testClient();
    render(<StackAutomationSettings />, { wrapper: wrapper(queryClient) });
    const anyAge = await screen.findByRole('checkbox', { name: /Also stack old channels, of any age/ });
    fireEvent.click(anyAge);
    await waitFor(() => expect(anyAge).toBeChecked());
    const days = screen.getByLabelText('Recent-frame requirement (days)');
    expect(days).toHaveValue(30);
    answer = current(true, 5, 15, true, 14);
    await act(() => queryClient.refetchQueries({ queryKey: ['stack-automation-settings'] }));
    await waitFor(() => expect(anyAge).not.toBeChecked());
    expect(days).toHaveValue(14);
  });
});
