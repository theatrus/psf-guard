import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import FlatChoicePicker from '../FlatChoicePicker';
import type { CalibrationKindExplanation } from '../../api/types';

// The Detail view moves to the next light without
// remounting the Calibration panel, so the picker's local state (the
// whole-session box, the group in the menu) must not carry to it.

const ok = <T,>(data: T) => HttpResponse.json({ success: true, data });

const change = {
  key: 'c-1',
  start: 1_789_200_000,
  end: 1_789_800_000,
  what: 'new dust',
  camera_side: true,
  filters: ['HA'],
  side: null,
  by: 'undetermined' as const,
  chosen: null,
  inside: true,
};

const kind = {
  kind: 'flat',
  master: { frames: 10, nights: 1 },
  used: [],
  matching_unused: [],
  refused: [],
  kept_out: [],
  suspect: [],
  other_filters: 0,
  other_cameras: 0,
  choice_options: { changes: [change], groups: [] },
} as unknown as CalibrationKindExplanation;

describe('a light’s flat choices, moving between lights', () => {
  it('does not carry “Every light of its imaging session” to the next light', async () => {
    const sent: unknown[] = [];
    server.use(
      http.get('/api/info', () => ok({ version: 'test', cache_directory: '/c', allow_database_management: true })),
      http.put('/api/db/main/calibrations/flat-choices', async ({ request }) => {
        sent.push(await request.json());
        return ok({ lights: 1, without_guid: 0 });
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(
      <QueryClientProvider client={client}>
        <FlatChoicePicker dbId="main" imageId={8} kind={kind} />
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Every light of its imaging session' }));
    // The next light, as ImageDetailView renders it: same instance.
    view.rerender(
      <QueryClientProvider client={client}>
        <FlatChoicePicker dbId="main" imageId={9} kind={kind} />
      </QueryClientProvider>,
    );
    fireEvent.click(within(await screen.findByTestId('calibration-flat-side')).getByRole('button', { name: 'After' }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ image_ids: [9], whole_session: false });
  });
});
