import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import StudioDocuments from '../StudioDocuments';
import type { StudioDocumentSummary } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => HttpResponse.json({ success: true, data, error: null });

const iris: StudioDocumentSummary = {
  id: '0a0e0ac0-d7a0-4a79-b361-9ef7086ac5b0',
  name: 'Iris',
  project_id: 1,
  target_id: 42,
  revision: 3,
  saved_at: 1_795_825_165,
  saved_by: 'ana',
  saved_from: 'Observatory Mac',
  preview: 'b'.repeat(64),
  exports: [{
    sha256: 'a'.repeat(64), revision: 2, name: 'Iris.tif', format: 'tif',
    size: 154_744_939, saved_at: 1_795_825_194, saved_by: 'ana',
  }],
};

function show(canWrite: boolean) {
  server.use(http.get('/api/db/test/projects/1/targets', () => ok([{
    id: 42, name: 'NGC 7023', active: true, image_count: 1, accepted_count: 1,
    rejected_count: 0, has_files: true,
  }])));
  render(<StudioDocuments dbId="test" projectId={1} canWrite={canWrite} />, { wrapper: wrapper() });
}

describe('StudioDocuments', () => {
  it('words a save on a server without accounts by the computer it came from', async () => {
    server.use(http.get('/api/db/test/studio-documents', () => ok([
      { ...iris, saved_by: 'this computer', saved_from: 'Observatory Mac' },
    ])));
    show(false);
    expect(await screen.findByText(/Revision 3, saved from Observatory Mac on /)).toBeInTheDocument();
  });

  it('lists a document with its exports and opens it in PSF Studio', async () => {
    server.use(http.get('/api/db/test/studio-documents', () => ok([iris])));
    show(false);
    expect(await screen.findByText('Iris')).toBeInTheDocument();
    expect(await screen.findByText(/NGC 7023 · Revision 3, saved by ana from Observatory Mac on /)).toBeInTheDocument();
    const exported = screen.getByRole('link', { name: 'Iris.tif' });
    expect(exported.getAttribute('href')).toMatch(/\/api\/db\/test\/studio-documents\/files\/a{64}$/);
    expect(screen.getByText('148 MB, from revision 2')).toBeInTheDocument();
    const open = screen.getByRole('link', { name: 'Open in PSF Studio' });
    expect(open.getAttribute('href')).toContain(`&document=${iris.id}`);
    // A viewer neither deletes nor saves setups.
    expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Save as a Color setup' })).toBeNull();
  });

  it('deletes a document after the confirmation names what goes', async () => {
    let documents = [iris];
    let deleted = '';
    server.use(
      http.get('/api/db/test/studio-documents', () => ok(documents)),
      http.delete('/api/db/test/studio-documents/:id', ({ params }) => {
        deleted = String(params.id);
        documents = [];
        return ok(null);
      }),
    );
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    show(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
    expect(confirm.mock.calls[0][0]).toMatch(/“Iris” and its saved revisions, and its 1 export\?/);
    expect(confirm.mock.calls[0][0]).toMatch(/cannot be undone/);
    await waitFor(() => expect(deleted).toBe(iris.id));
    await waitFor(() => expect(screen.queryByText('Iris')).toBeNull());
    confirm.mockRestore();
  });

  it('saves a document as a Color setup, or explains why it cannot', async () => {
    let saved: unknown = null;
    let convertible = true;
    server.use(
      http.get('/api/db/test/studio-documents', () => ok([iris])),
      http.get('/api/db/test/studio-documents/:id/processing', () => (convertible
        ? ok({ background_extraction: null })
        : HttpResponse.json(
          { success: false, data: null, error: 'This document cannot be a Color processing setup: a layer mask' },
          { status: 400 },
        ))),
      http.post('/api/processing-setups', async ({ request }) => {
        saved = await request.json();
        return ok({ name: 'Iris look', kind: 'color', settings: {}, created_unix_seconds: 1, updated_unix_seconds: 1 });
      }),
    );
    show(true);
    fireEvent.click(await screen.findByRole('button', { name: 'Save as a Color setup' }));
    fireEvent.change(screen.getByLabelText('Setup name'), { target: { value: 'Iris look' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save setup' }));
    expect(await screen.findByText('Saved as the Color setup “Iris look”.')).toBeInTheDocument();
    expect(saved).toEqual({ name: 'Iris look', kind: 'color', settings: { background_extraction: null } });

    convertible = false;
    fireEvent.click(screen.getByRole('button', { name: 'Save as a Color setup' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save setup' }));
    expect(await screen.findByText(/cannot be a Color processing setup: a layer mask/)).toBeInTheDocument();
  });
});
