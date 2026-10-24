import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { ProcessingSetupsDocument } from '../../api/types';
import ProcessingSetupsManager from '../ProcessingSetupsManager';

function listing(defaultColor: string): ProcessingSetupsDocument {
  return {
    schema_version: 1,
    default_color: defaultColor,
    setups: [
      { name: 'Bright', kind: 'view', settings: {}, created_unix_seconds: 5, updated_unix_seconds: 5 },
      { name: 'Default', kind: 'color', settings: {}, created_unix_seconds: 0, updated_unix_seconds: 0 },
      { name: 'Faint', kind: 'color', settings: {}, created_unix_seconds: 5, updated_unix_seconds: 5 },
    ],
  };
}

function renderManager() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(
    <QueryClientProvider client={queryClient}><ProcessingSetupsManager /></QueryClientProvider>
  );
}

function row(name: string) {
  return screen.getByRole('cell', { name: new RegExp(`^${name}`) }).closest('tr')!;
}

describe('ProcessingSetupsManager', () => {
  it('shows which Color setup new color previews start from, and keeps it from being deleted', async () => {
    server.use(http.get('/api/processing-setups', () => HttpResponse.json({
      success: true, data: listing('Default'), error: null,
    })));
    renderManager();
    await screen.findByText('Faint');

    const seeded = row('Default');
    expect(within(seeded).getByText('Default for new color previews')).toBeInTheDocument();
    expect(within(seeded).getByText('Not edited')).toBeInTheDocument();
    expect(within(seeded).getByRole('button', { name: 'Delete' })).toBeDisabled();
    expect(within(seeded).queryByRole('button', { name: /default for new color previews/ }))
      .not.toBeInTheDocument();

    expect(within(row('Faint')).getByRole('button', { name: 'Delete' })).toBeEnabled();
    expect(within(row('Faint')).getByRole('button', {
      name: 'Make “Faint” the default for new color previews',
    })).toBeEnabled();
    // A view setup cannot be the default for color previews.
    expect(within(row('Bright')).queryByRole('button', { name: /default/ })).not.toBeInTheDocument();
  });

  it('marks another Color setup as the default', async () => {
    let marked = 'Default';
    const requests: string[] = [];
    server.use(
      http.get('/api/processing-setups', () => HttpResponse.json({
        success: true, data: listing(marked), error: null,
      })),
      http.put('/api/processing-setups/:name/default', ({ params }) => {
        requests.push(String(params.name));
        marked = String(params.name);
        return HttpResponse.json({ success: true, data: listing(marked), error: null });
      }),
    );
    renderManager();
    await userEvent.click(await screen.findByRole('button', {
      name: 'Make “Faint” the default for new color previews',
    }));
    expect(requests).toEqual(['Faint']);
    expect(await screen.findByText('New color previews now start from “Faint”')).toBeInTheDocument();
    await waitFor(() =>
      expect(within(row('Faint')).getByText('Default for new color previews')).toBeInTheDocument()
    );
    expect(within(row('Default')).getByRole('button', { name: 'Delete' })).toBeEnabled();
    expect(within(row('Faint')).getByRole('button', { name: 'Delete' })).toBeDisabled();
  });

  afterEach(() => vi.restoreAllMocks());

  it('leaves the unedited Default out of Export all and says when an import skips one', async () => {
    server.use(
      http.get('/api/processing-setups', () => HttpResponse.json({
        success: true, data: listing('Default'), error: null,
      })),
      http.post('/api/processing-setups/import', () => HttpResponse.json({
        success: true, error: null,
        data: { imported: 1, replaced: 0, skipped: 1, setups: listing('Default').setups },
      })),
    );
    const exported: Blob[] = [];
    vi.spyOn(URL, 'createObjectURL').mockImplementation((blob) => {
      exported.push(blob as Blob);
      return 'blob:setups';
    });
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined);
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    const view = renderManager();
    await screen.findByText('Faint');
    await userEvent.click(screen.getByRole('button', { name: 'Export all' }));
    const document_ = JSON.parse(await exported[0].text()) as ProcessingSetupsDocument;
    expect(document_.setups.map((setup) => setup.name)).toEqual(['Bright', 'Faint']);

    const input = view.container.querySelector('input[type="file"]') as HTMLInputElement;
    await userEvent.upload(input, new File([JSON.stringify(document_)], 'setups.json', {
      type: 'application/json',
    }));
    expect(await screen.findByText(
      'Imported 1 new, replaced 0, skipped 1 unedited default setup already here'
    )).toBeInTheDocument();
  });
});
