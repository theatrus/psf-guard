import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AstroBinFilterMapDialog from '../AstroBinFilterMapDialog';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

describe('AstroBinFilterMapDialog', () => {
  it('edits entries with night ranges, refuses bad rows, and saves the whole map', async () => {
    let saved: unknown = null;
    server.use(
      http.get('/api/db/alpha/astrobin/filters', () =>
        HttpResponse.json(
          ok({
            entries: [
              { id: 1, filter_name: 'G', astrobin_id: 10, label: 'Old G', to_night: '2025-12-31' },
            ],
          })
        )
      ),
      http.put('/api/db/alpha/astrobin/filters', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(ok({ entries: [] }));
      })
    );
    render(
      <AstroBinFilterMapDialog dbId="alpha" dbName="Alpha" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    expect(await screen.findByLabelText('Filter name, row 1')).toHaveValue('G');
    expect(screen.getByLabelText('Last night, row 1')).toHaveValue('2025-12-31');

    fireEvent.click(screen.getByRole('button', { name: '+ Add entry' }));
    const save = screen.getByRole('button', { name: 'Save map' });
    expect(save).toBeDisabled();
    expect(screen.getByText('needs a filter name')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Filter name, row 2'), { target: { value: 'G' } });
    // A pasted equipment page address stands for its id.
    fireEvent.change(screen.getByLabelText('AstroBin id, row 2'), {
      target: { value: 'https://app.astrobin.com/equipment/explorer/filter/11/chroma-g' },
    });
    expect(screen.getByRole('link', { name: 'Open filter 11 on AstroBin' })).toHaveAttribute(
      'href',
      'https://app.astrobin.com/equipment/explorer/filter/11'
    );
    fireEvent.change(screen.getByLabelText('First night, row 2'), { target: { value: 'soon' } });
    expect(screen.getByText('nights are YYYY-MM-DD')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('First night, row 2'), {
      target: { value: '2026-01-01' },
    });
    fireEvent.change(screen.getByLabelText('Label, row 2'), { target: { value: 'New G' } });
    expect(save).toBeEnabled();
    fireEvent.click(save);

    await waitFor(() =>
      expect(saved).toEqual({
        entries: [
          { filter_name: 'G', astrobin_id: 10, label: 'Old G', to_night: '2025-12-31' },
          { filter_name: 'G', astrobin_id: 11, label: 'New G', from_night: '2026-01-01' },
        ],
      })
    );
  });

  it('is read-only without management', async () => {
    server.use(
      http.get('/api/db/alpha/astrobin/filters', () =>
        HttpResponse.json(ok({ entries: [{ id: 1, filter_name: 'L', astrobin_id: 4049 }] }))
      )
    );
    render(
      <AstroBinFilterMapDialog dbId="alpha" dbName="Alpha" canManage={false} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByLabelText('Filter name, row 1')).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Save map' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '+ Add entry' })).not.toBeInTheDocument();
  });
});
