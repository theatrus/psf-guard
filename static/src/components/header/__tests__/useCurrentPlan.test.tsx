import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import type { DirectorPlanRow } from '../../../api/directorTypes';
import { useCurrentPlan } from '../useCurrentPlan';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const row: DirectorPlanRow = {
  project: { id: 'plan-id', name: 'Imported field', revision: 1 },
  links: [{ catalog_slug: 'images', catalog_name: 'Images', source_row_id: 7,
    source_name: 'Imported field', source_project_guid: 'project-guid',
    rig: { id: 'rig-id', name: 'Rig', revision: 1 }, source_state: 0,
    earliest_capture_s: null, latest_capture_s: null, targets: [] }],
  progress: null, framing: null, plan: null, activation: null,
};

function Entry() {
  const plan = useCurrentPlan();
  return <output data-testid="entry">{plan.loading ? 'loading' : plan.href ?? 'no project'}</output>;
}

function mount(route: string, rows: DirectorPlanRow[] = []) {
  server.use(
    http.get('/api/director/v1/status', () => ok({ enabled: true, protocol_version: 1, instance_id: 'instance' })),
    http.get('/api/director/v1/plans', () => ok({ rows, warnings: [] })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}><Entry /></MemoryRouter></QueryClientProvider>);
}

describe('Planning entry for the current project', () => {
  it('opens the source project when it has no plan, keeping its database and review scope', async () => {
    mount('/grid?db=images&project=7&target=9&filter=Ha');
    await waitFor(() => expect(screen.getByTestId('entry')).toHaveTextContent('/plan?db=images&project=7&target=9&filter=Ha&plan=images%3A7'));
  });

  it('opens the canonical plan when the project was adopted', async () => {
    mount('/grid?db=images&project=7', [row]);
    await waitFor(() => expect(screen.getByTestId('entry')).toHaveTextContent('/plan?db=images&project=7&plan=project-guid'));
  });

  it('requires a project rather than picking another plan in the database', async () => {
    mount('/grid?db=images', [row]);
    await waitFor(() => expect(screen.getByTestId('entry')).toHaveTextContent('no project'));
  });
});
