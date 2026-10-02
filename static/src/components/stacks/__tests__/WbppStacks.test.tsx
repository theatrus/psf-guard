import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import WbppStacks from '../WbppStacks';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function entry(targetId: number, target: string, filter: string, drizzle = false) {
  return {
    job_id: `${filter}`.padEnd(64, 'a'),
    artifact_revision: 'wbpp-1',
    accepted_only: false,
    created_unix_seconds: 1,
    wbpp: {
      master_file: `masterLight_BIN-1_EXPOSURE-75.00s_FILTER-${filter}_mono_autocrop.xisf`,
      output_dir: '/runs/ngc7331/wbpp-out',
      exposure_seconds: 75,
      drizzle,
      autocrop: true,
      imported_unix_seconds: 1,
    },
    group: {
      index: 0, target_id: targetId, target_name: target, filter_name: filter, state: 'ready', phase: 'ready',
      preview_url: `/api/db/test/stack-previews/x/0/preview?v=${filter}`, fits_url: `/api/db/test/stack-previews/x/0/fits?v=${filter}`,
      frames: [], input_images: [],
    },
  };
}

function renderSection(targetId: number | null = null, canImport = true) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return render(<WbppStacks dbId="test" projectId={1} targetId={targetId} canImport={canImport} />, { wrapper: Wrapper });
}

describe('WbppStacks', () => {
  it('lists the masters WBPP made, for the target in scope', async () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/wbpp', () =>
        ok({ schema_version: 1, database_id: 'test', project_id: 1, updated_unix_seconds: 1, groups: [
          entry(10, 'NGC 7331', 'L'),
          entry(10, 'NGC 7331', 'R', true),
          entry(11, 'Panel 2', 'L'),
        ] })
      )
    );
    renderSection(10);
    expect(await screen.findByText('NGC 7331 · L')).toBeInTheDocument();
    expect(screen.getByText('NGC 7331 · R')).toBeInTheDocument();
    expect(screen.queryByText('Panel 2 · L')).toBeNull();
    expect(screen.getByText('75 s subs · drizzled · cropped')).toBeInTheDocument();
    expect(screen.getAllByRole('link', { name: 'FITS' })).toHaveLength(2);
  });

  it('offers no import without database management', async () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/wbpp', () =>
        ok({ schema_version: 1, database_id: 'test', project_id: 1, updated_unix_seconds: 1, groups: [] })
      )
    );
    renderSection(null, false);
    expect(await screen.findByText(/No WBPP stacks yet/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Take in the last run' })).toBeNull();
  });

  it('takes in the last run and says what it did', async () => {
    let imported = false;
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/wbpp', () =>
        ok({ schema_version: 1, database_id: 'test', project_id: 1, updated_unix_seconds: 1,
          groups: imported ? [entry(10, 'NGC 7331', 'L')] : [] })
      ),
      http.post('/api/db/:dbId/projects/:projectId/stack-previews/wbpp/import', () => {
        imported = true;
        return ok({ imported: ['masterLight_L.xisf'], skipped: [] });
      })
    );
    renderSection();
    expect(await screen.findByText(/No WBPP stacks yet/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Take in the last run' }));
    expect(await screen.findByText('Took in 1 master.')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('NGC 7331 · L')).toBeInTheDocument());
  });
});
