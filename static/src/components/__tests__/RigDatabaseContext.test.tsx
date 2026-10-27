import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import TauriSettings from '../TauriSettings';
import { openSettings, settingsDatabaseOf, OPEN_SETTINGS_EVENT } from '../../utils/settingsIntent';

const ok = (data: unknown) => ({ success: true, data, error: null });
function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter>{children}</MemoryRouter></QueryClientProvider>;
}

describe('Database context in Settings', () => {
  it('saves file settings without resending the remote connection', async () => {
    const changes: Record<string, unknown>[] = [];
    server.use(
      http.get('/api/info', () => HttpResponse.json(ok({ allow_database_management: true }))),
      http.get('/api/databases', () => HttpResponse.json(ok([
        { id: 'c925', name: 'C925', database_path: '/tmp/c925.sqlite', image_directories: ['/images'],
          remote_image_upload: { enabled: true, token_configured: true, image_directory: '/incoming' } },
      ]))),
      http.put('/api/databases/c925', async ({ request }) => {
        changes.push(await request.json() as Record<string, unknown>);
        return HttpResponse.json(ok({}));
      }),
    );
    render(<TauriSettings isOpen initialIntent="databases" initialDatabaseId="c925" onClose={() => {}} />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: 'Edit C925' }));
    fireEvent.change(screen.getByLabelText('Display name (optional):'), { target: { value: 'C925 observatory' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() => expect(changes).toHaveLength(1));
    expect(changes[0]).toMatchObject({ name: 'C925 observatory', db_path: '/tmp/c925.sqlite', image_dirs: ['/images'] });
    expect(changes[0]).not.toHaveProperty('remote_image_upload');
    expect(changes[0]).not.toHaveProperty('ts_sync');
  });

  it('carries an explicit registration slug in a Settings shortcut', () => {
    let chosen: string | null = null;
    const receive = (event: Event) => { chosen = settingsDatabaseOf(event); };
    window.addEventListener(OPEN_SETTINGS_EVENT, receive);
    try {
      openSettings('sync', { databaseId: 'c925' });
      expect(chosen).toBe('c925');
      openSettings('rigs');
      expect(chosen).toBeNull();
      expect(settingsDatabaseOf(new Event(OPEN_SETTINGS_EVENT))).toBeNull();
    } finally { window.removeEventListener(OPEN_SETTINGS_EVENT, receive); }
  });
});
