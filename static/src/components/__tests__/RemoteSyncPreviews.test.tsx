import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import RemoteSyncPreviews from '../RemoteSyncPreviews';

const databases = [{ id: 'review', name: 'Review copy' }];

const counts = {
  inserted: 0,
  updated: 0,
  unchanged: 0,
  skipped: 0,
};

function previewEntry(overrides: Record<string, unknown> = {}) {
  return {
    preview_id: 'preview-1',
    kind: 'push_grades',
    source: 'telescope-catalog',
    created_at: 1_000,
    expires_at: Math.floor(Date.now() / 1000) + 900,
    result: {
      kind: 'push_grades',
      dry_run: true,
      source_db_id: 'telescope-catalog',
      destination_db_id: 'review',
      exposuretemplate: counts,
      project: counts,
      ruleweight: counts,
      target: counts,
      exposureplan: counts,
      acquiredimage: { ...counts, updated: 42 },
      imagedata: null,
      grades: { changed: 42, unchanged: 3 },
      grade_filled: 40,
      grade_preserved: 2,
      imagedata_bytes: 0,
      total_inserted: 0,
      total_updated: 42,
    },
    ...overrides,
  };
}

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

describe('RemoteSyncPreviews', () => {
  it('lists staged previews with their source and counts', async () => {
    server.use(
      http.get('/api/databases/review/sync/previews', () => ok([previewEntry()]))
    );
    render(<RemoteSyncPreviews databases={databases} />, { wrapper: wrapper() });

    expect(await screen.findByText('Grades push')).toBeInTheDocument();
    expect(screen.getByText('telescope-catalog')).toBeInTheDocument();
    expect(screen.getByText(/42 updated/)).toBeInTheDocument();
    expect(screen.getByText(/42 grades/)).toBeInTheDocument();
  });

  it('renders nothing when no previews are staged', async () => {
    server.use(http.get('/api/databases/review/sync/previews', () => ok([])));
    const { container } = render(<RemoteSyncPreviews databases={databases} />, {
      wrapper: wrapper(),
    });
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it('applies a preview and reloads the list', async () => {
    let applied = 0;
    let listCalls = 0;
    server.use(
      http.get('/api/databases/review/sync/previews', () => {
        listCalls++;
        return ok(listCalls === 1 ? [previewEntry()] : []);
      }),
      http.post('/api/databases/review/sync/previews/preview-1/apply', () => {
        applied++;
        return ok(previewEntry().result);
      })
    );
    render(<RemoteSyncPreviews databases={databases} />, { wrapper: wrapper() });

    await userEvent.click(await screen.findByRole('button', { name: 'Apply' }));

    await waitFor(() => expect(applied).toBe(1));
    expect(await screen.findByText('Apply succeeded')).toBeInTheDocument();
  });

  it('discards a preview', async () => {
    let discarded = 0;
    server.use(
      http.get('/api/databases/review/sync/previews', () => ok([previewEntry()])),
      http.delete('/api/databases/review/sync/previews/preview-1', () => {
        discarded++;
        return ok(true);
      })
    );
    render(<RemoteSyncPreviews databases={databases} />, { wrapper: wrapper() });

    await userEvent.click(await screen.findByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(discarded).toBe(1));
  });

  it('surfaces an apply failure without dropping the row', async () => {
    server.use(
      http.get('/api/databases/review/sync/previews', () => ok([previewEntry()])),
      http.post('/api/databases/review/sync/previews/preview-1/apply', () =>
        HttpResponse.json(
          { success: false, data: null, error: 'destination changed since preview', status: null },
          { status: 409 }
        )
      )
    );
    render(<RemoteSyncPreviews databases={databases} />, { wrapper: wrapper() });

    await userEvent.click(await screen.findByRole('button', { name: 'Apply' }));
    expect(await screen.findByText(/destination changed|Request failed/)).toBeInTheDocument();
    expect(screen.getByText('Grades push')).toBeInTheDocument();
  });
});
