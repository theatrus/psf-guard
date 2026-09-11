import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectExposureGrouping from '../ProjectExposureGrouping';

function mount(canManage = true) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const mounted = render(<QueryClientProvider client={queryClient}>
    <ProjectExposureGrouping dbId="test" projectId={1} canManage={canManage} />
  </QueryClientProvider>);
  return { ...mounted, queryClient };
}

describe('project exposure grouping', () => {
  it('saves the project setting and restores it on reload', async () => {
    let enabled = false;
    server.use(
      http.get('/api/db/test/projects/1/processing-settings', () => HttpResponse.json({
        success: true, data: { split_exposure_groups: enabled },
      })),
      http.put('/api/db/test/projects/1/processing-settings', async ({ request }) => {
        enabled = (await request.json() as { split_exposure_groups: boolean }).split_exposure_groups;
        return HttpResponse.json({ success: true, data: { split_exposure_groups: enabled } });
      }),
    );
    const first = mount();
    const toggle = screen.getByRole('checkbox', { name: 'Separate exposure groups' });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
    await userEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
    expect(enabled).toBe(true);
    first.unmount();
    mount();
    await waitFor(() => expect(screen.getByRole('checkbox')).toBeChecked());
  });

  it('retains the server value and shows a save failure', async () => {
    server.use(
      http.get('/api/db/test/projects/1/processing-settings', () => HttpResponse.json({
        success: true, data: { split_exposure_groups: false },
      })),
      http.put('/api/db/test/projects/1/processing-settings', () => HttpResponse.json(
        { error: 'Database is read-only' }, { status: 403 }
      )),
    );
    mount();
    const toggle = screen.getByRole('checkbox');
    await waitFor(() => expect(toggle).toBeEnabled());
    await userEvent.click(toggle);
    expect(await screen.findByRole('alert')).toHaveTextContent('Database is read-only');
    expect(toggle).not.toBeChecked();
    expect(toggle).toBeEnabled();
  });

  it('exposes the persisted state without allowing a viewer to change it', async () => {
    server.use(http.get('/api/db/test/projects/1/processing-settings', () => HttpResponse.json({
      success: true, data: { split_exposure_groups: true },
    })));
    mount(false);
    await waitFor(() => expect(screen.getByRole('checkbox')).toBeChecked());
    expect(screen.getByRole('checkbox')).toBeDisabled();
  });
});
