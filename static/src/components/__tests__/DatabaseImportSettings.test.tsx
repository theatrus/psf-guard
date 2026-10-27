import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import DatabaseImportSettings from '../DatabaseImportSettings';

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}>{children}</QueryClientProvider>;
}
const database = { id: 'c925', name: 'C925', db_path: '/data/c925.sqlite', image_dirs: ['/images'] };

describe('Database import settings', () => {
  it('edits exclusion patterns directly and saves only the import rules', async () => {
    const updates: unknown[] = [];
    server.use(http.put('/api/databases/c925', async ({ request }) => {
      updates.push(await request.json());
      return HttpResponse.json({ success: true, data: {} });
    }));
    const preview = vi.fn();
    const saved = vi.fn(async () => undefined);
    render(<DatabaseImportSettings database={database} canManage importing={false} onSaved={saved} onPreview={preview} onFiles={() => undefined} />, { wrapper });
    expect(screen.getByLabelText('Exclude:')).toBeEnabled();
    fireEvent.change(screen.getByLabelText('Exclude:'), { target: { value: '**/Hocus Focus/**' } });
    expect(screen.getByRole('button', { name: 'Preview import' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Save import settings' }));
    await waitFor(() => expect(updates).toEqual([{ scan_rules: {
      exclude: ['**/Hocus Focus/**'], include: [], lights_must_say_light: false,
      require_object: false, min_exposure_seconds: null, match_by_position: true,
    } }]));
    expect(saved).toHaveBeenCalledOnce();
    expect(preview).not.toHaveBeenCalled();
  });

  it('saves scheduling separately without sending database paths or scan rules', async () => {
    const updates: unknown[] = [];
    server.use(http.put('/api/databases/c925', async ({ request }) => {
      updates.push(await request.json());
      return HttpResponse.json({ success: true, data: {} });
    }));
    render(<DatabaseImportSettings database={database} canManage importing={false} onSaved={async () => undefined} onPreview={() => undefined} onFiles={() => undefined} />, { wrapper });
    fireEvent.click(screen.getByRole('checkbox', { name: 'Automatically import new frames' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save import settings' }));
    await waitFor(() => expect(updates).toEqual([{ autoimport: {
      enabled: true, on_open: true, interval_minutes: 0, scope: 'all', backfill: true,
      accept_other_rigs: false, new_targets: 'ask',
    } }]));
  });

  it('allows a preview of configured folders but protects edits without database management', () => {
    const preview = vi.fn();
    render(<DatabaseImportSettings database={database} canManage={false} importing={false} onSaved={async () => undefined} onPreview={preview} onFiles={() => undefined} />, { wrapper });
    expect(screen.getByLabelText('Exclude:')).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Save import settings' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Preview import' }));
    expect(preview).toHaveBeenCalledOnce();
  });
});
