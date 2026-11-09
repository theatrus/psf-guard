import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import PluginsOnRig from '../rigs/PluginsOnRig';

const ok = (data: unknown) => ({ success: true, data, error: null });
const upload = (change: Record<string, unknown>) => ({
  enabled: false, sync_enabled: false, token_configured: false, key_configured: false, clients: [],
  directory_template: '%TARGET%/%TYPE%/%FILTER%', directory_template_source: 'preset', directory_template_samples: 0, ...change,
});
const pluginView = (clients: number, work: string) => ({
  coordinator_instance_id: 'i', catalog_id: 'c', rig_id: 'r', profile_revision: 1, configuration: null, filter_names: {}, camera_defaults: null,
  clients: Array.from({ length: clients }, (_, i) => ({ client_id: `${i}`, profile_id: 'p', client_name: 'NINA', created_at_ms: 1, report: null, review: 'none' })),
  plans: [], policy: null, work, left_out: [], grant: null, last_contact_at_ms: null,
});

function show(receiver: Record<string, unknown>, director?: { clients: number; work: string }, canWrite = true) {
  server.use(
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'rig', name: 'Rig', image_directories: [], remote_image_upload: upload(receiver) }]))),
    http.get('/api/director/v1/catalogs/rig/rig/plugin', () => HttpResponse.json(ok(pluginView(director?.clients ?? 0, director?.work ?? 'not_allowed')))),
  );
  const onChangeMode = vi.fn();
  const access = { ...({} as ReturnType<typeof useAccess>), canWrite };
  render(<AccessContext.Provider value={access}>
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <PluginsOnRig databaseId="rig" director={director ? { mode: 'target_scheduler', onChangeMode } : undefined} />
    </QueryClientProvider>
  </AccessContext.Provider>);
  return { onChangeMode };
}

describe('Plugins on this rig', () => {
  it('shows each plugin on or off with what it does', async () => {
    const { onChangeMode } = show(
      { token_configured: true, sync_enabled: true, enabled: true, image_directory: '/images/rig', clients: [{ client_uuid: 'a', name: 'PC', paired_at: 1 }] },
      { clients: 1, work: 'off' },
    );
    const sync = await screen.findByTestId('plugin-sync');
    await waitFor(() => expect(sync).toHaveTextContent('PSF Guard SyncOnscheduler sync'));
    // Uploads serve both plugins, in a row of their own.
    expect(screen.getByTestId('plugin-uploads')).toHaveTextContent('Image uploadsOnto /images/rig');
    const director = screen.getByTestId('plugin-director');
    await waitFor(() => expect(director).toHaveTextContent('PSF Guard DirectorOffPaired, work off'));
    expect(sync.querySelector('.plugin-state')).toHaveAttribute('data-on', 'true');
    expect(director.querySelector('.plugin-state')).toHaveAttribute('data-on', 'false');
    expect(screen.getByText(/Picks the work: Target Scheduler/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Change mode' }));
    expect(onChangeMode).toHaveBeenCalled();
  });

  it('says a plugin that is paired but does nothing is off, and one never paired is not paired', async () => {
    show({ token_configured: true, clients: [{ client_uuid: 'a', name: 'PC', paired_at: 1 }] }, { clients: 0, work: 'not_allowed' });
    await waitFor(() => expect(screen.getByTestId('plugin-sync')).toHaveTextContent('OffPaired, scheduler sync off'));
    expect(screen.getByTestId('plugin-uploads')).toHaveTextContent('OffNeither plugin sends image files');
    await waitFor(() => expect(screen.getByTestId('plugin-director')).toHaveTextContent('OffNot paired'));
  });

  it('shows only the Sync plugin without planning', async () => {
    show({});
    expect(await screen.findByTestId('plugin-sync')).toHaveTextContent('Not paired');
    expect(screen.queryByTestId('plugin-director')).toBeNull();
    expect(screen.queryByText(/Picks the work/)).toBeNull();
  });

  it('tells a read-only viewer the Director plugin is shown to editors', async () => {
    show({}, { clients: 1, work: 'ready' }, false);
    expect(await screen.findByTestId('plugin-director')).toHaveTextContent('PSF Guard DirectorShown to editors');
  });
});
