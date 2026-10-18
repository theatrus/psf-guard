import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import type { BackupJob, BackupsOverview } from '../../api/backupTypes';
import BackupSettings from '../BackupSettings';

const desktop = vi.hoisted(() => ({ isTauri: false }));
vi.mock('../../utils/tauri', () => ({
  isTauriApp: () => desktop.isTauri,
  getServerUrl: async () => '',
  tauriFileSystem: { pickPath: vi.fn(async () => '/picked') },
}));

afterEach(() => {
  desktop.isTauri = false;
});

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

function ok(data: unknown, status = 200) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' }, { status });
}

const NAME = 'psf-guard-backup-2026-10-16T130000Z-full-auto.zip';

function overview(change: Partial<BackupsOverview> = {}): BackupsOverview {
  return {
    settings: {
      enabled: false,
      frequency: 'daily',
      weekday: 6,
      time: '13:00',
      destination: null,
      scope: 'full',
      include_secrets: false,
      retention: { kind: 'count', count: 7 },
    },
    suggested_destination: '/home/astro/Documents/psf-guard/backups',
    job: null,
    last_run: null,
    next_run_at: null,
    destination_warnings: [],
    backups: [],
    secrets: { peer_keys: 2, collaboration_credentials: true, server_config_tokens: 0 },
    meta_bytes: 40_000,
    catalog_bytes: 120_000_000,
    catalogs: 2,
    unreachable_catalogs: [],
    ...change,
  };
}

const running: BackupJob = {
  trigger: 'manual', scope: 'full', include_secrets: false, destination: '/srv/cache/backup-downloads',
  location: 'downloads', stage: 'copying', current: 'catalog imaging-rig', files_done: 3,
  files_total: 9, bytes_done: 50, bytes_total: 200, waiting_for: null, started_at: 1_790_000_000,
  stopping: false,
};

function serve(view: () => BackupsOverview, status: () => { job: BackupJob | null } = () => ({ job: null })) {
  server.use(
    http.get('/api/backups', () => ok(view())),
    http.get('/api/backups/status', () => ok({ last_run: null, ...status() })),
    http.post('/api/backups/check-destination', async ({ request }) => {
      const { path } = (await request.json()) as { path: string };
      return ok({
        error: null,
        exists: true,
        warnings: path.startsWith('/rig')
          ? ['This backup folder is inside the catalog folder for Imaging Rig (/rig/schedulerdb.sqlite). A disk failure could destroy both the catalog and its backups.']
          : [],
      });
    })
  );
}

describe('BackupSettings', () => {
  it('backs up now for download, then shows the run with a stop', async () => {
    let job: BackupJob | null = null;
    let asked: unknown = null;
    let stopped = false;
    serve(() => overview(), () => ({ job }));
    server.use(
      http.post('/api/backups/run', async ({ request }) => {
        asked = await request.json();
        job = running;
        return ok({ job, last_run: null }, 202);
      }),
      http.post('/api/backups/stop', () => {
        stopped = true;
        job = { ...running, stopping: true };
        return ok({ job, last_run: null });
      })
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Back up now' });
    expect(within(card).getByText(/all 2 catalogs/)).toHaveTextContent('About 120.0 MB');
    expect(within(card).getByText(/Never offered as a download/)).toBeInTheDocument();
    expect(within(card).getByLabelText(/Save on the server/)).toHaveValue('');
    fireEvent.click(within(card).getByRole('button', { name: 'Back up now' }));
    await waitFor(() =>
      expect(asked).toEqual({ scope: 'full', include_secrets: false, destination: null })
    );
    const progress = await within(card).findByRole('progressbar', { name: 'Backup progress' });
    expect(progress).toHaveAttribute('aria-valuenow', '25');
    expect(within(card).getByText('Copying catalog imaging-rig · 3/9 files')).toBeInTheDocument();
    const button = within(card).getByRole('button', { name: 'Back up now' });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription('A backup is running.');
    // The bar is not a live region; only the stage is read out.
    expect(progress.closest('[role="status"], [aria-live]')).toBeNull();
    expect(card.querySelector('[aria-live="polite"]')).toHaveTextContent('Backup: Copying');
    fireEvent.click(within(card).getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(stopped).toBe(true));
    expect(await within(card).findByText('Stopping…')).toBeInTheDocument();
  });

  it('backs up settings only, with secrets, to a server folder and warns beside a catalog', async () => {
    let asked: unknown = null;
    serve(() => overview());
    server.use(
      http.post('/api/backups/run', async ({ request }) => {
        asked = await request.json();
        return ok({ job: running, last_run: null }, 202);
      })
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Back up now' });
    fireEvent.click(within(card).getByRole('radio', { name: /Settings only/ }));
    // Secrets are never offered as a download.
    const secrets = within(card).getByRole('checkbox', { name: /Include secrets/ });
    expect(secrets).toBeDisabled();
    expect(within(card).getByText(/Never offered as a download/)).toBeInTheDocument();
    fireEvent.change(within(card).getByLabelText(/Save on the server/), { target: { value: '/rig/backups' } });
    expect(secrets).toBeEnabled();
    expect(within(card).getByText(/2 peer keys, collaboration sign-ins/)).toBeInTheDocument();
    fireEvent.click(secrets);
    expect(await within(card).findByText(/inside the catalog folder for Imaging Rig/)).toBeInTheDocument();
    fireEvent.click(within(card).getByRole('button', { name: 'Back up now' }));
    await waitFor(() =>
      expect(asked).toEqual({ scope: 'meta', include_secrets: true, destination: '/rig/backups' })
    );
  });

  it('lists archives with a download link in the browser, and the last run with its file', async () => {
    const listing = {
      file_name: NAME, bytes: 52_000_000, created_at: '2026-10-16T13:00:00Z', created_unix: 1_791_000_000,
      scope: 'full' as const, trigger: 'schedule' as const, secrets: 'redacted' as const,
      psf_guard_version: '0.11.0', catalogs: 2, problems: 0, location: 'destination' as const,
      download_path: `/backups/files/destination/${NAME}`,
    };
    const SECRET = 'psf-guard-backup-2026-10-15T130000Z-meta.zip';
    serve(() => overview({
      backups: [listing, {
        ...listing, file_name: SECRET, scope: 'meta', trigger: 'manual', secrets: 'included', bytes: 13_000,
        created_unix: 1_790_900_000, download_path: `/backups/files/destination/${SECRET}`,
      }],
      last_run: {
        trigger: 'schedule', scope: 'full', started_at: 1_791_000_000, finished_at: 1_791_000_060,
        outcome: 'succeeded', location: 'destination', destination: '/mnt/nas/backups', file_name: NAME,
        bytes: 52_000_000, error: null, problems: ['catalog observatory: /mnt/obs/db.sqlite is not reachable'],
        pruned: ['old.zip'], prune_kept: ['older-with-observatory.zip'],
        prune_kept_for: ['Observatory Rig'], prune_skipped: null,
      },
    }));
    render(<BackupSettings />, { wrapper: wrapper() });
    const list = await screen.findByRole('region', { name: 'Backups' });
    const link = within(list).getByRole('link', { name: `Download ${NAME}` });
    expect(link).toHaveAttribute('href', `/api/backups/files/destination/${NAME}`);
    expect(within(list).getByText(/Settings and 2 catalogs · scheduled/)).toBeInTheDocument();
    expect(within(list).getByText('52.0 MB')).toBeInTheDocument();
    const now = screen.getByRole('region', { name: 'Back up now' });
    expect(within(now).getByText(/Removed 1 older/)).toBeInTheDocument();
    expect(within(now).getByText(
      'Kept 1 older backup because it includes Observatory Rig, which this backup does not include.'
    )).toBeInTheDocument();
    // An archive with secrets is never offered as a download.
    expect(within(list).queryByRole('link', { name: `Download ${SECRET}` })).not.toBeInTheDocument();
    expect(within(list).getByText(/Download unavailable: this backup contains credentials/)).toBeInTheDocument();
    expect(screen.getByText(/succeeded, 52.0 MB/)).toBeInTheDocument();
  });

  it('saves to a folder in the desktop app, with no download links', async () => {
    desktop.isTauri = true;
    let asked: unknown = null;
    serve(() => overview({
      backups: [{
        file_name: NAME, bytes: 1, created_at: '', created_unix: 1, scope: 'meta', trigger: 'manual',
        secrets: 'redacted', psf_guard_version: '0.11.0', catalogs: 0, problems: 0,
        location: 'destination', download_path: `/backups/files/destination/${NAME}`,
      }],
    }));
    server.use(
      http.post('/api/backups/run', async ({ request }) => {
        asked = await request.json();
        return ok({ job: running, last_run: null }, 202);
      })
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Back up now' });
    const folder = within(card).getByLabelText(/Save to/);
    expect(folder).toHaveValue('/home/astro/Documents/psf-guard/backups');
    expect(within(card).getByRole('button', { name: 'Browse…' })).toBeInTheDocument();
    fireEvent.change(folder, { target: { value: '' } });
    expect(within(card).getByRole('button', { name: 'Back up now' })).toBeDisabled();
    fireEvent.change(folder, { target: { value: '/Volumes/Backup/psf' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Back up now' }));
    await waitFor(() =>
      expect(asked).toEqual({ scope: 'full', include_secrets: true, destination: '/Volumes/Backup/psf' })
    );
    expect(screen.queryByRole('link', { name: /Download/ })).not.toBeInTheDocument();
  });

  it('turns the schedule on with the suggested folder and saves when and how many to keep', async () => {
    let saved: unknown = null;
    serve(() => overview());
    server.use(
      http.put('/api/backups/settings', async ({ request }) => {
        saved = await request.json();
        return ok(overview({
          settings: { ...(saved as BackupsOverview['settings']), since: 1 },
          next_run_at: 1_900_000_000,
        }));
      })
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Scheduled backups' });
    expect(within(card).getByText('Not scheduled')).toBeInTheDocument();
    const save = within(card).getByRole('button', { name: 'Save schedule' });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription('No changes to save.');
    fireEvent.click(within(card).getByRole('checkbox', { name: /Back up on a schedule/ }));
    expect(within(card).getByLabelText(/^Folder/)).toHaveValue('/home/astro/Documents/psf-guard/backups');
    fireEvent.change(within(card).getByLabelText('Every'), { target: { value: 'weekly' } });
    fireEvent.change(within(card).getByLabelText('On'), { target: { value: '2' } });
    fireEvent.change(within(card).getByLabelText('At'), { target: { value: '12:30' } });
    fireEvent.change(within(card).getByLabelText('Keep'), { target: { value: 'age' } });
    fireEvent.change(within(card).getByLabelText('Keep (days)'), { target: { value: '0' } });
    expect(save).toHaveAccessibleDescription('Keep a whole number from 1 to 3650.');
    fireEvent.change(within(card).getByLabelText('Keep (days)'), { target: { value: '30' } });
    // Each control is named by the words shown beside it.
    expect(within(card).getByRole('combobox', { name: 'Every' })).toHaveValue('weekly');
    expect(within(card).getByRole('combobox', { name: 'On' })).toHaveValue('2');
    expect(within(card).getByLabelText('At')).toHaveAccessibleDescription("The server's local time.");
    fireEvent.click(save);
    await waitFor(() =>
      expect(saved).toEqual({
        enabled: true,
        frequency: 'weekly',
        weekday: 2,
        time: '12:30',
        destination: '/home/astro/Documents/psf-guard/backups',
        scope: 'full',
        include_secrets: false,
        retention: { kind: 'age', days: 30 },
      })
    );
    expect(await within(card).findByText(/2030/)).toBeInTheDocument();
  });

  it('starts a never-saved schedule with secrets in the desktop app', async () => {
    desktop.isTauri = true;
    let saved: unknown = null;
    serve(() => overview());
    server.use(
      http.put('/api/backups/settings', async ({ request }) => {
        saved = await request.json();
        return ok(overview({ settings: { ...(saved as BackupsOverview['settings']), since: 1 } }));
      })
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Scheduled backups' });
    expect(within(card).getByRole('checkbox', { name: /Include secrets/ })).toBeChecked();
    const save = within(card).getByRole('button', { name: 'Save schedule' });
    expect(save).toHaveAccessibleDescription('No changes to save.');
    fireEvent.click(within(card).getByRole('checkbox', { name: /Back up on a schedule/ }));
    fireEvent.click(save);
    await waitFor(() => expect(saved).toMatchObject({ enabled: true, include_secrets: true }));
  });

  it("says what the server refused about a schedule", async () => {
    serve(() => overview());
    server.use(
      http.put('/api/backups/settings', () =>
        HttpResponse.json(
          { success: false, data: null, error: 'the backup folder must be an absolute path' },
          { status: 400 }
        )
      )
    );
    render(<BackupSettings />, { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'Scheduled backups' });
    fireEvent.click(within(card).getByRole('checkbox', { name: /Back up on a schedule/ }));
    fireEvent.change(within(card).getByLabelText(/^Folder/), { target: { value: 'relative' } });
    fireEvent.click(within(card).getByRole('button', { name: 'Save schedule' }));
    expect(await within(card).findByRole('alert')).toHaveTextContent('must be an absolute path');
  });
});
