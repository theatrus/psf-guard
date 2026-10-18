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
            seen: [],
          })
        )
      ),
      http.put('/api/db/alpha/astrobin/filters', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json(ok({ entries: [], seen: [] }));
      })
    );
    render(
      <AstroBinFilterMapDialog dbId="alpha" dbName="Alpha" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    expect(await screen.findByLabelText('Filter name, row 1')).toHaveValue('G');
    expect(screen.getByLabelText('Last night, row 1')).toHaveValue('2025-12-31');
    // An open bound says so; Safari would otherwise draw today's date in the empty field.
    expect(screen.getByLabelText('Last night, row 1')).toHaveClass('is-set');
    expect(screen.getByLabelText('First night, row 1')).not.toHaveClass('is-set');
    expect(screen.getByText('· any')).toBeInTheDocument();

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
    fireEvent.blur(screen.getByLabelText('AstroBin id, row 2'));
    expect(screen.getByLabelText('AstroBin id, row 2')).toHaveValue('11');
    fireEvent.change(screen.getByLabelText('First night, row 2'), {
      target: { value: '2026-01-01' },
    });
    fireEvent.change(screen.getByLabelText('Last night, row 2'), {
      target: { value: '2025-06-01' },
    });
    expect(screen.getByText('ends before it starts')).toBeInTheDocument();
    expect(screen.getByLabelText('Last night, row 2')).toHaveAttribute('aria-invalid', 'true');
    fireEvent.change(screen.getByLabelText('Last night, row 2'), { target: { value: '' } });
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

  it("offers the catalog's filter names that have no entry, with the server-wide id", async () => {
    server.use(
      http.get('/api/db/alpha/astrobin/filters', () =>
        HttpResponse.json(
          ok({
            entries: [{ id: 1, filter_name: 'G', astrobin_id: 10 }],
            seen: [
              { filter_name: 'g', frames: 40, first_night: '2025-03-01', last_night: '2026-09-12' },
              { filter_name: 'Ha', frames: 1, first_night: '2026-04-16', last_night: '2026-04-16' },
              { filter_name: 'OIII', frames: 12, first_night: '2026-01-02', last_night: '2026-02-03' },
            ],
          })
        )
      ),
      http.get('/api/settings/astrobin', () => HttpResponse.json(ok({ filter_ids: { ha: 4049 } })))
    );
    render(
      <AstroBinFilterMapDialog dbId="alpha" dbName="Alpha" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // G is mapped already, whatever its case; its row says when it was used.
    const offered = await screen.findByRole('group', { name: 'Filters not in the map' });
    expect(screen.getByText('40 lights, 2025-03-01 to 2026-09-12')).toBeInTheDocument();
    expect(
      Array.from(offered.querySelectorAll('.astrobin-filter-map-chip')).map((b) => b.textContent)
    ).toEqual(['+ Ha', '+ OIII']);

    fireEvent.click(await screen.findByRole('button', { name: 'Add Ha, 1 light on 2026-04-16' }));
    expect(screen.getByLabelText('Filter name, row 2')).toHaveValue('Ha');
    await waitFor(() => expect(screen.getByLabelText('AstroBin id, row 2')).toHaveValue('4049'));
    expect(screen.getByRole('button', { name: 'Save map' })).toBeEnabled();

    fireEvent.click(screen.getByRole('button', { name: 'Add OIII, 12 lights, 2026-01-02 to 2026-02-03' }));
    expect(screen.getByLabelText('Filter name, row 3')).toHaveValue('OIII');
    expect(screen.getByLabelText('AstroBin id, row 3')).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('button', { name: 'Save map' })).toBeDisabled();
    expect(screen.queryByRole('group', { name: 'Filters not in the map' })).not.toBeInTheDocument();
  });

  it('is read-only without management', async () => {
    server.use(
      http.get('/api/db/alpha/astrobin/filters', () =>
        HttpResponse.json(ok({ entries: [{ id: 1, filter_name: 'L', astrobin_id: 4049 }], seen: [] }))
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
