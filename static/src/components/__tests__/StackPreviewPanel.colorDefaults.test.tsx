import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorProcessing } from '../../api/types';
import StackPreviewPanel from '../StackPreviewPanel';
import { ALL_COLOR_ROLES, defaultColorProcessing } from '../stackColorProcessing';

const images = [
  { id: 1, target_id: 42, target_name: 'M 27', filter_name: 'R', grading_status: 1 },
  { id: 2, target_id: 42, target_name: 'M 27', filter_name: 'R', grading_status: 1 },
];

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function renderPanel() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <MemoryRouter initialEntries={['/stacks?db=test&project=1']}>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </MemoryRouter>
    );
  }
  return render(
    <StackPreviewPanel dbId="test" projectId={1} images={images} selectionSource="visible"
      onOpenImage={() => undefined} />,
    { wrapper: Wrapper }
  );
}

function mockBuild(defaultColorProcessing?: StackColorProcessing) {
  const submitted: Record<string, unknown>[] = [];
  server.use(
    http.get('/api/stack-activity', () => ok({ schema_version: 1, active: [] })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({ groups: [] })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({ targets: [], jobs: [] })),
    http.get('/api/processing-setups', () => ok({
      schema_version: 1,
      setups: [],
      default_color: defaultColorProcessing ? 'Faint' : null,
      ...(defaultColorProcessing ? { default_color_processing: defaultColorProcessing } : {}),
    })),
    http.post('/api/db/:dbId/projects/:projectId/stack-previews/channels', async ({ request }) => {
      submitted.push(await request.json() as Record<string, unknown>);
      return ok([]);
    }),
  );
  return submitted;
}

describe('StackPreviewPanel color defaults', () => {
  it('asks for a first color preview with the default Color setup the server marks', async () => {
    const faint: StackColorProcessing = {
      ...defaultColorProcessing(ALL_COLOR_ROLES),
      background_extraction: null,
    };
    const submitted = mockBuild(faint);
    renderPanel();
    await userEvent.click(await screen.findByRole('button', { name: 'Build stacks' }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0].color_defaults).toEqual(faint);
  });

  it('uses the built-in pipeline from a server that marks no setup', async () => {
    const submitted = mockBuild();
    renderPanel();
    await userEvent.click(await screen.findByRole('button', { name: 'Build stacks' }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0].color_defaults).toEqual(defaultColorProcessing(ALL_COLOR_ROLES));
  });
});
