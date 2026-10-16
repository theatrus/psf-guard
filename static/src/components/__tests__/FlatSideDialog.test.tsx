import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import FlatSideDialog from '../FlatSideDialog';

const ok = <T,>(data: T) => HttpResponse.json({ success: true, data });

describe('the Images bulk action for flat sides', () => {
  it('lists the changes the selected lights were shot during and puts them on a side', async () => {
    const sent: unknown[] = [];
    let asked: unknown = null;
    server.use(
      http.post('/api/db/main/calibrations/flat-choices/options', async ({ request }) => {
        asked = await request.json();
        return ok({
          changes: [{
            key: 'c-1', rig_uuid: 'rig', start: 1_789_200_000, end: 1_789_800_000, what: 'new dust: 1 shadow near (1423, 3545)',
            camera_side: true, filters: ['HA', 'OIII'], lights: 3, chosen_before: 0, chosen_after: 3,
          }],
          with_choices: 3,
          without_guid: 0,
        });
      }),
      http.put('/api/db/main/calibrations/flat-choices', async ({ request }) => {
        sent.push(await request.json());
        return ok({ lights: 3, without_guid: 0 });
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <FlatSideDialog dbId="main" imageIds={[3, 1, 2]} onClose={() => {}} />
      </QueryClientProvider>,
    );
    const change = await screen.findByTestId('flat-side-change');
    expect(asked).toEqual({ image_ids: [3, 1, 2] });
    expect(change).toHaveTextContent('3 selected lights shot during it · all put after');
    expect(within(change).getByRole('button', { name: 'After' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(within(change).getByRole('button', { name: 'Before' }));
    await waitFor(() => expect(sent).toEqual([{ image_ids: [3, 1, 2], choice: 'before', change_key: 'c-1' }]));
    fireEvent.click(screen.getByTestId('flat-side-reset-all'));
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]).toEqual({ image_ids: [3, 1, 2], choice: 'reset' });
  });
});
