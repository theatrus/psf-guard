import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import FlatChoicePicker from '../FlatChoicePicker';
import type { CalibrationKindExplanation } from '../../api/types';

const ok = <T,>(data: T) => HttpResponse.json({ success: true, data });

const change = {
  key: 'c-1',
  start: 1_789_200_000,
  end: 1_789_800_000,
  what: 'new dust: 1 shadow near (1423, 3545)',
  camera_side: true,
  filters: ['HA', 'OIII'],
  side: null,
  by: 'undetermined' as const,
  chosen: null,
  inside: true,
};

function flat(extra: Partial<CalibrationKindExplanation> = {}): CalibrationKindExplanation {
  return {
    kind: 'flat',
    master: { frames: 10, nights: 1 },
    used: [],
    matching_unused: [],
    refused: [],
    kept_out: [],
    suspect: [],
    other_filters: 0,
    other_cameras: 0,
    basis: {
      basis: 'undetermined',
      event: { key: 'c-1', start: change.start, end: change.end, dust: 0.016, broad: 0, camera_side: true, filters: ['ha'], what: change.what },
      placement: { by: 'undetermined', session: null, pixels: null },
    },
    choice_options: {
      changes: [change],
      groups: [
        { response_key: 'r-old', family: 'HA at 0.0°', first_night: '2026-09-10', last_night: '2026-09-12', sessions: 2, current: true, chosen: false },
        { response_key: 'r-new', family: 'HA at 0.0°', first_night: '2026-09-20', last_night: '2026-09-20', sessions: 1, current: false, chosen: false },
      ],
    },
    ...extra,
  };
}

/** Render the picker; `manage` is the server's database-management gate. */
function show(kind: CalibrationKindExplanation, manage = true) {
  const sent: unknown[] = [];
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: manage })),
    http.put('/api/db/main/calibrations/flat-choices', async ({ request }) => {
      sent.push(await request.json());
      return ok({ lights: 1, without_guid: 0 });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <FlatChoicePicker dbId="main" imageId={8} kind={kind} />
    </QueryClientProvider>,
  );
  return sent;
}

describe('a light’s flat choices', () => {
  it('puts a light shot during a change on a side, or its whole imaging session', async () => {
    const sent = show(flat());
    const side = await screen.findByTestId('calibration-flat-side');
    expect(side).toHaveTextContent('side undetermined');
    fireEvent.click(screen.getByRole('checkbox', { name: 'Every light of its imaging session' }));
    fireEvent.click(within(side).getByRole('button', { name: 'After' }));
    await waitFor(() => expect(sent).toEqual([
      { image_ids: [8], whole_session: true, choice: 'after', change_key: 'c-1' },
    ]));
  });

  it('gives a light one group’s flats', async () => {
    const sent = show(flat());
    fireEvent.change(await screen.findByTestId('calibration-flat-group'), { target: { value: 'r-new' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use group' }));
    await waitFor(() => expect(sent).toEqual([
      { image_ids: [8], whole_session: false, choice: 'group', group_response: 'r-new' },
    ]));
  });

  it('says what a person chose, marks the side pressed, and resets it', async () => {
    const sent = show(flat({
      basis: undefined,
      choices: [{ choice: 'after', change_key: 'c-1', label: 'after the change of 2026-09-12', chosen_at: 1 }],
      choice_options: { changes: [{ ...change, chosen: 'after', by: 'undetermined' }], groups: [] },
    }));
    const chosen = await screen.findByTestId('calibration-flat-chosen');
    expect(chosen).toHaveTextContent('Put after the change of 2026-09-12');
    expect(await screen.findByRole('button', { name: 'After' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(within(chosen).getByRole('button', { name: 'Reset' }));
    await waitFor(() => expect(sent).toEqual([
      { image_ids: [8], whole_session: false, choice: 'reset', change_key: 'c-1' },
    ]));
  });

  it('shows a choice whose change is gone as orphaned, never as applied', async () => {
    show(flat({
      choices: [{
        choice: 'before',
        change_key: 'gone',
        label: 'before the change of 2026-08-11',
        chosen_at: 1,
        orphaned: 'the change it was made for is gone',
      }],
    }));
    const orphaned = await screen.findByTestId('calibration-flat-orphaned');
    expect(orphaned).toHaveTextContent('the change it was made for is gone');
    expect(screen.queryByTestId('calibration-flat-chosen')).toBeNull();
  });

  it('lets a viewer see the choices but not change them', async () => {
    show(flat({
      choices: [{ choice: 'group', label: 'the group HA at 0.0°, 2026-09-20', chosen_at: 1 }],
    }), false);
    const chosen = await screen.findByTestId('calibration-flat-chosen');
    expect(chosen).toHaveTextContent('Takes the group HA at 0.0°, 2026-09-20');
    // The gate is read once the server answers; still no buttons after.
    await waitFor(() => expect(screen.queryByRole('button')).toBeNull());
    expect(screen.queryByTestId('calibration-flat-side')).toBeNull();
  });
});
