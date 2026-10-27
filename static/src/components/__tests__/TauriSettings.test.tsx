import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { HttpResponse, http } from 'msw';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext } from '../../auth/access';
import type { AuthUserSummary } from '../../api/types';
import TauriSettings from '../TauriSettings';
import UnsavedMark from '../UnsavedMark';

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
    },
  });

  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}><MemoryRouter>{children}</MemoryRouter></QueryClientProvider>;
  };
}

describe('TauriSettings import state', () => {
  it('shows user management as its own tab when server login is enabled', async () => {
    let users: AuthUserSummary[] = [
      { username: 'editor', role: 'read_write' },
      { username: 'reviewer', role: 'read_only', email: 'reviewer@example.com' },
    ];
    server.use(
      http.get('/api/auth/users', () =>
        HttpResponse.json({
          success: true,
          data: users,
          error: null,
          status: 'ready',
        })
      ),
      http.post('/api/auth/users', async ({ request }) => {
        const body = await request.json() as {
          username: string;
          role: 'read_only' | 'read_write';
          email?: string;
          password: string;
        };
        users = [...users, {
          username: body.username,
          role: body.role,
          email: body.email,
        }];
        return HttpResponse.json({
          success: true,
          data: users,
          error: null,
          status: 'ready',
        });
      })
    );

    const Wrapper = createWrapper();
    render(
      <AccessContext.Provider
        value={{
          status: {
            authentication_required: true,
            authenticated: true,
            role: 'read_write',
            username: 'editor',
            can_compute: true,
          },
          canWrite: true,
          canCompute: true,
          logout: async () => undefined,
        }}
      >
        <TauriSettings isOpen onClose={() => undefined} />
      </AccessContext.Provider>,
      { wrapper: Wrapper }
    );

    const usersTab = await screen.findByRole('tab', { name: 'Users' });
    expect(screen.queryByRole('tab', { name: 'Agents (MCP)' })).not.toBeInTheDocument();
    fireEvent.click(usersTab);
    expect(
      await screen.findByRole('heading', { name: 'Browser users' })
    ).toBeInTheDocument();
    expect(await screen.findByText('reviewer')).toBeInTheDocument();
    expect(await screen.findByText('reviewer@example.com')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '+ Add user' }));
    fireEvent.change(screen.getByLabelText('Username'), {
      target: { value: 'viewer' },
    });
    fireEvent.change(screen.getByLabelText('Email (optional)'), {
      target: { value: 'viewer@example.com' },
    });
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'long-viewer-password' },
    });
    fireEvent.change(screen.getByLabelText('Confirm password'), {
      target: { value: 'long-viewer-password' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save user' }));

    expect(await screen.findByText('viewer')).toBeInTheDocument();
    expect(await screen.findByText('viewer@example.com')).toBeInTheDocument();
  });

  it('opens rig setup from the parent list and keeps exposure templates separate', async () => {
    const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
    server.use(
      http.get('/api/director/v1/status', () => HttpResponse.json({ success: true, data: { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true }, error: null })),
      http.get('/api/databases', () => HttpResponse.json({ success: true, data: [{ id: 'c925', name: 'C925', path: '/c925.sqlite' }], error: null })),
      http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json({ success: true, data: [{ rig, catalog_slug: 'c925', catalog_name: 'C925', profile: null, field_of_view: { width_degrees: 0.7, height_degrees: 0.5, pixel_scale_arcsec: 0.41, focal_ratio: 10 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } }], error: null })),
      http.get('/api/director/v1/rigs/status', () => HttpResponse.json({ success: true, data: [], error: null })),
      http.get('/api/director/v1/templates', () => HttpResponse.json({ success: true, data: [], error: null })),
    );
    render(<TauriSettings isOpen onClose={() => {}} initialIntent="rigs" />, { wrapper: createWrapper() });
    const tabs = await screen.findByRole('tab', { name: 'Overview' });
    expect(tabs).toHaveAttribute('aria-selected', 'true');
    expect(tabs).toHaveAccessibleDescription('Databases & rigs');
    expect(await screen.findByRole('tab', { name: 'Exposure templates' })).toHaveAccessibleDescription('Presets');
    fireEvent.click(await screen.findByRole('button', { name: 'Open C925' }));
    expect(screen.getByRole('navigation', { name: 'Rig database location' })).toHaveTextContent('C925');
    expect(screen.getByRole('tab', { name: 'Rig setup' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.click(screen.getByRole('tab', { name: 'Exposure templates' }));
    expect(await screen.findByRole('region', { name: 'Exposure template library' })).toBeInTheDocument();
  });

  it('keeps disk use and storage folders on their own Storage tab', async () => {
    server.use(
      http.get('/api/databases', () => HttpResponse.json({ success: true, data: [], error: null })),
      http.get('/api/settings/storage', () =>
        HttpResponse.json({
          success: true,
          error: null,
          data: {
            max_volume_percent: 90,
            stack_max_volume_percent: null,
            calibration_max_volume_percent: null,
            default_max_volume_percent: 90,
            min_max_volume_percent: 0,
            volumes: [],
            folders: [],
            folder_notes: [],
            can_choose_folders: true,
          },
        })
      ),
    );
    render(<TauriSettings isOpen onClose={() => {}} initialIntent="stacking" />, { wrapper: createWrapper() });
    expect(await screen.findByRole('tab', { name: 'Stacking' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByRole('heading', { name: 'Disk use' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('tab', { name: 'Folders & limits' }));

    expect(await screen.findByRole('heading', { name: 'Disk use' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Folders' })).toBeInTheDocument();
  });

  it('keeps a requested Planning page selected while its availability loads', async () => {
    let release = () => {};
    const ready = new Promise<void>(resolve => { release = resolve; });
    server.use(
      http.get('/api/director/v1/status', async () => {
        await ready;
        return HttpResponse.json({ success: true, error: null, data: { protocol_version: 1, enabled: true } });
      }),
      http.get('/api/director/v1/templates', () => HttpResponse.json({ success: true, error: null, data: [] })),
    );
    render(<TauriSettings isOpen initialIntent="templates" onClose={() => {}} />, { wrapper: createWrapper() });
    try {
      expect(screen.getByLabelText('Settings section')).toHaveValue('templates');
      expect(screen.getByRole('option', { name: 'Exposure templates — Loading…' })).toBeDisabled();
    } finally { release(); }
    expect(await screen.findByRole('tab', { name: 'Exposure templates' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByLabelText('Settings section')).toHaveValue('templates');
  });

  it('keeps processor use on its own Performance tab', async () => {
    server.use(
      http.get('/api/databases', () => HttpResponse.json({ success: true, data: [], error: null })),
      http.get('/api/settings/workers', () =>
        HttpResponse.json({
          success: true,
          error: null,
          data: {
            interactive_ratio: 0.5,
            background_ratio: 0.25,
            default_interactive_ratio: 0.5,
            default_background_ratio: 0.25,
            logical_cores: 16,
          },
        })
      ),
    );
    render(<TauriSettings isOpen onClose={() => {}} initialIntent="stacking" />, { wrapper: createWrapper() });
    expect(await screen.findByRole('tab', { name: 'Stacking' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByRole('heading', { name: 'Processor use' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('tab', { name: 'CPU & memory' }));

    expect(await screen.findByRole('heading', { name: 'Processor use' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Background work share of cores' })).toBeInTheDocument();
  });

  it('moves GPU and integrations to focused pages while keeping old section shortcuts', async () => {
    const gpu = { enabled: true, debayer: true, detection: true, resampling: true, accumulator: true, integration: true, concurrency: 1, trace: false };
    server.use(http.get('/api/settings/stacking/gpu', () => HttpResponse.json({ success: true, error: null, data: { gpu, recommended: gpu, max_concurrency: 16 } })));
    const { rerender } = render(<TauriSettings isOpen initialIntent="stacking" onClose={() => {}} />, { wrapper: createWrapper() });
    expect(await screen.findByRole('tab', { name: 'GPU' })).toHaveAccessibleDescription('Performance');
    expect(screen.queryByRole('checkbox', { name: /^Use the GPU/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'GPU' }));
    expect(await screen.findByRole('checkbox', { name: /^Use the GPU/ })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Stacking method' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('tab', { name: 'Processing setups' }));
    expect(screen.getByRole('tab', { name: 'Processing setups' })).toHaveAccessibleDescription('Presets');
    expect(screen.queryByRole('heading', { name: 'PixInsight' })).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'AstroBin' })).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'PixInsight' })).toHaveAccessibleDescription('Exports & integrations');
    fireEvent.click(screen.getByRole('tab', { name: 'PixInsight' }));
    expect(await screen.findByRole('heading', { name: 'PixInsight' })).toBeInTheDocument();

    rerender(<TauriSettings isOpen initialIntent="catalogs" initialField="gaia_photometry" onClose={() => {}} />);
    expect(await screen.findByRole('tab', { name: 'Reference catalogs' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Reference catalogs' })).toHaveAccessibleDescription('Storage & backups');
  });

  it('does not block section navigation for an unsaved form behind Settings', async () => {
    render(<><TauriSettings isOpen onClose={() => {}} /><UnsavedMark label="Background rig profile" dirty /></>, { wrapper: createWrapper() });
    fireEvent.click(await screen.findByRole('tab', { name: 'GPU' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'GPU' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.change(screen.getByLabelText('Settings section'), { target: { value: 'stacking' } });
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Stacking' })).toHaveAttribute('aria-selected', 'true');
  });

  it('keeps a neutral Close action in the header without a global save, reload or restart', async () => {
    const onClose = vi.fn();
    render(<TauriSettings isOpen onClose={onClose} />, { wrapper: createWrapper() });
    const actions = await screen.findByRole('group', { name: 'Settings actions' });
    expect(within(actions).getByRole('button', { name: 'Close settings' })).toHaveTextContent('Close');
    expect(screen.queryByRole('button', { name: /^(Done|Reload Page|Restart Server)$/i })).not.toBeInTheDocument();
    fireEvent.click(within(actions).getByRole('button', { name: 'Close settings' }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('asks before closing Settings with unsaved edits', async () => {
    const onClose = vi.fn();
    const { rerender } = render(
      <><TauriSettings isOpen onClose={onClose} /><UnsavedMark label="C925 setup" dirty /></>,
      { wrapper: createWrapper() },
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Close settings' }));
    const ask = screen.getByRole('alertdialog');
    expect(ask).toHaveTextContent('Unsaved changes in C925 setup. Close without saving them?');
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(within(ask).getByRole('button', { name: 'Keep editing' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Close settings' }));
    fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Close without saving' }));
    expect(onClose).toHaveBeenCalledTimes(1);

    // With the edits saved, closing needs no confirmation.
    rerender(<><TauriSettings isOpen onClose={onClose} /><UnsavedMark label="C925 setup" dirty={false} /></>);
    fireEvent.click(screen.getByRole('button', { name: 'Close settings' }));
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  it('keeps calibration settings on their own tab, not under Setups', async () => {
    render(<TauriSettings isOpen onClose={() => {}} />, { wrapper: createWrapper() });
    fireEvent.click(await screen.findByRole('tab', { name: 'Processing setups' }));
    expect(document.querySelector('.calibration-matching-settings')).toBeNull();
    fireEvent.click(screen.getByRole('tab', { name: 'Calibration' }));
    expect(await screen.findByRole('heading', { name: 'Calibration', level: 3 })).toBeInTheDocument();
  });

  it('shows a server without accounts how to connect an agent, with no token', async () => {
    render(<TauriSettings isOpen onClose={() => {}} />, { wrapper: createWrapper() });
    fireEvent.click(await screen.findByRole('tab', { name: 'Agents (MCP)' }));
    const url = `${window.location.origin}/api/mcp`;
    expect(await screen.findByText(`claude mcp add --transport http psf-guard ${url}`)).toBeInTheDocument();
    expect(screen.queryByText(/PSF_GUARD_TOKEN/)).not.toBeInTheDocument();
  });

  it('hides catalog management on a read-only server', async () => {
    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(
      await screen.findByText('🔒 Database management is read-only')
    ).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Seiza Catalogs' })).not.toBeInTheDocument();
  });

  it('resumes a server-side import scan when settings opens', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'archive',
              name: 'Archive',
              database_path: '/tmp/archive.sqlite',
              image_directories: ['/images'],
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true,
              stage: 'scanning',
              image_dirs: ['/images'],
              total_files: 120,
              scanned_files: 37,
              outcome: null,
              started_at: 1,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Archive' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Database & files' }));
    expect(await screen.findByText('Scanning headers… 37/120')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Importing…' })).toBeDisabled();
  });

  it('shows database quality work as a separate background job', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'archive',
              name: 'Archive',
              database_path: '/tmp/archive.sqlite',
              image_directories: ['/images'],
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running: false,
              stage: '',
              image_dirs: ['/images'],
              total_files: 0,
              scanned_files: 0,
              outcome: null,
              started_at: null,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/archive/analysis/quality-backfill', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: true,
            progress: {
              running: true,
              force: false,
              total_targets: 4,
              processed_targets: 1,
              current_target_id: 42,
              started_at: 1,
              finished_at: null,
            },
          },
          error: null,
          status: 'ready',
        })
      )
    );

    server.use(http.get('/api/settings/processing', () => HttpResponse.json({ success: true, data: { defaults: { analyze_new_frames: false, pair_calibrated_copies: true, scan_calibrated_copies: false }, databases: [{ id: 'archive', name: 'Archive', overrides: { analyze_new_frames: null, pair_calibrated_copies: null, scan_calibrated_copies: null }, effective: { analyze_new_frames: false, pair_calibrated_copies: true, scan_calibrated_copies: false } }] } })));
    render(<TauriSettings initialIntent="review" isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    fireEvent.click(await screen.findByText('Archive · Uses global settings'));
    expect(
      await screen.findByText(/Analyzing quality in the background… 1\/4 targets/)
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Import' })).not.toBeInTheDocument();
  });

  it('edits the per-database remote image receiver without exposing its token', async () => {
    const savedUpdates: Array<{
      placement?: string;
      directory_template?: string;
      directory_template_source?: string;
      rescan_directory_layout?: boolean;
    }> = [];
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'remote',
              name: 'Remote catalog',
              database_path: '/tmp/remote.sqlite',
              image_directories: ['/images/remote'],
              remote_image_upload: {
                enabled: true,
                image_directory: '/images/remote',
                token_configured: true,
                sync_enabled: false,
                placement: 'target_tree',
                directory_template: '%TARGET%/%DATE%/%TYPE%',
                catalog_directory_template: '2026/%TARGET%/%NIGHT%/%TYPE%',
                directory_template_source: 'catalog',
                directory_template_samples: 18,
              },
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/remote/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running: false,
              stage: '',
              image_dirs: [],
              total_files: 0,
              scanned_files: 0,
              outcome: null,
              started_at: null,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      ),
      http.put('/api/databases/remote', async ({ request }) => {
        const body = await request.json() as {
          remote_image_upload?: {
            placement?: string;
            directory_template?: string;
            rescan_directory_layout?: boolean;
          };
        };
        expect(Object.keys(body)).toEqual(['remote_image_upload']);
        const update = body.remote_image_upload ?? {};
        savedUpdates.push(update);
        return HttpResponse.json({
          success: true,
          data: {
            id: 'remote',
            name: 'Remote catalog',
            database_path: '/tmp/remote.sqlite',
            image_directories: ['/images/remote'],
            remote_image_upload: {
              enabled: true,
              image_directory: '/images/remote',
              token_configured: true,
              sync_enabled: false,
              placement: update.placement,
              directory_template:
                update.directory_template ?? '%YEAR%/%TARGET%/%NIGHT%/%TYPE%',
              directory_template_source: 'preset',
              directory_template_samples: 0,
            },
          },
          error: null,
          status: 'ready',
        });
      })
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Remote catalog' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Database & files' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Sync' }));
    fireEvent.click(screen.getByRole('button', { name: 'Edit remote receive and pairing' }));
    expect(
      screen.getByRole('checkbox', { name: 'Accept remote image uploads' })
    ).toBeChecked();
    expect(screen.getByLabelText('Receive directory:')).toHaveValue('/images/remote');
    expect(screen.getByLabelText('Folder layout:')).toHaveValue('target_tree');
    // A catalog match in force shows as the selected choice, with the
    // remembered fallback available as a preset below it.
    const directoryTemplateSelect = screen.getByLabelText('Layout:');
    expect(directoryTemplateSelect).toHaveValue('catalog');
    expect(
      screen.getByText('Detected from 18 database images:')
    ).toBeInTheDocument();
    expect(
      screen.getByText('2026/%TARGET%/%NIGHT%/%TYPE%')
    ).toBeInTheDocument();
    expect(
      within(directoryTemplateSelect)
        .getAllByRole('option')
        .map((option) => option.textContent)
    ).toEqual([
      'Match database: 2026/%TARGET%/%NIGHT%/%TYPE%',
      '%YEAR%/%TARGET%/%NIGHT%/%TYPE%',
      '%TARGET%/%NIGHT%/%TYPE%',
      '%TARGET%/%DATE%/%TYPE%',
      '%TARGET%/%TYPE%/%FILTER%',
      '%NIGHT%/%TARGET%/%TYPE%',
      'Custom',
    ]);
    expect(screen.getByLabelText('Remote API key:')).toHaveAttribute(
      'placeholder',
      'Unchanged'
    );

    fireEvent.click(screen.getByRole('button', { name: 'Generate' }));
    const generatedToken = screen.getByLabelText('Remote API key:');
    expect(generatedToken).toHaveAttribute('type', 'text');
    expect((generatedToken as HTMLInputElement).value).toMatch(/^[0-9a-f]{64}$/);
    expect(
      screen.getByText('Copy this key now. It will not be shown again after saving.')
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy' })).toBeEnabled();

    // Queue a rescan first, then choose Custom: the choice cancels the
    // rescan, because a scan would only find the same match again.
    const rescanButton = screen.getByRole('button', {
      name: 'Rescan database layout',
    });
    fireEvent.click(rescanButton);
    expect(rescanButton).toHaveAttribute('aria-pressed', 'true');
    fireEvent.change(screen.getByLabelText('Layout:'), {
      target: { value: 'custom' },
    });
    expect(rescanButton).toHaveAttribute('aria-pressed', 'false');
    fireEvent.change(screen.getByLabelText('Custom database layout:'), {
      target: { value: '%TARGET%/%DATE%/%TYPE%/%FILTER%' },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Save connection settings' }));
    await waitFor(() => expect(savedUpdates).toHaveLength(1));
    // Choosing Custom is a choice of layout (#399): the save says so, and
    // the server will keep it rather than re-scanning the catalog over it.
    expect(savedUpdates[0]).toMatchObject({
      placement: 'target_tree',
      directory_template: '%TARGET%/%DATE%/%TYPE%/%FILTER%',
      directory_template_source: 'preset',
    });
    expect(savedUpdates[0].rescan_directory_layout).toBeUndefined();

    await screen.findByText('Saved.');
    fireEvent.click(screen.getByRole('button', { name: 'Edit remote receive and pairing' }));
    expect(
      screen.getByRole('button', { name: 'Rescan database layout' })
    ).toHaveAttribute('aria-pressed', 'false');

    fireEvent.change(screen.getByLabelText('Folder layout:'), {
      target: { value: 'flat' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save connection settings' }));
    await waitFor(() => expect(savedUpdates).toHaveLength(2));
    expect(savedUpdates[1].placement).toBe('flat');
    expect(savedUpdates[1].directory_template).toBeUndefined();
    expect(savedUpdates[1].rescan_directory_layout).toBeUndefined();
  });

  it('keeps the editor with the selected database in a multi-database list', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'c925',
              name: 'C925',
              database_path: '/tmp/c925.sqlite',
              image_directories: ['/images/c925'],
            },
            {
              id: 'ultracat',
              name: 'Ultra Cat',
              database_path: '/tmp/ultracat.sqlite',
              image_directories: ['/images/ultracat'],
            },
          ],
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/db/:dbId/import', () =>
        HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running: false,
              stage: '',
              image_dirs: [],
              total_files: 0,
              scanned_files: 0,
              outcome: null,
              started_at: null,
              finished_at: null,
              error: null,
            },
          },
          error: null,
          status: 'ready',
        })
      ),
      http.delete('/api/databases/:dbId', () =>
        HttpResponse.json({
          success: true,
          data: { removed: true },
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Open C925' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Database & files' }));
    const c925 = screen.getByRole('region', { name: 'C925' });
    fireEvent.click(within(c925).getByRole('button', { name: 'Edit C925' }));
    expect(within(c925).getByText('Editing database')).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'Ultra Cat' })).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Display name (optional):'), { target: { value: 'Changed C925' } });
    fireEvent.change(screen.getByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/not-added' } });
    fireEvent.click(screen.getByRole('button', { name: 'Databases & rigs' }));
    expect(screen.getByText(/Unsaved changes in database files/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
    expect(screen.getByLabelText('Display name (optional):')).toHaveValue('Changed C925');
    fireEvent.click(screen.getByRole('tab', { name: 'Sync' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard changes and switch' }));
    expect(screen.queryByLabelText('Name:')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Databases & rigs' }));
    fireEvent.click(screen.getByRole('button', { name: 'Open Ultra Cat' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Database & files' }));
    const ultraCat = screen.getByRole('region', { name: 'Ultra Cat' });
    fireEvent.click(within(ultraCat).getByRole('button', { name: 'Edit Ultra Cat' }));
    expect(screen.getByLabelText('Display name (optional):')).toHaveValue('Ultra Cat');
    expect(screen.getByPlaceholderText('Type an absolute path and press Add')).toHaveValue('');

    const confirmRemove = vi.spyOn(window, 'confirm').mockReturnValue(true);
    fireEvent.click(within(ultraCat).getByRole('button', { name: 'Remove Ultra Cat' }));
    confirmRemove.mockRestore();
    await screen.findByText('Removed Ultra Cat.');

    fireEvent.click(screen.getByRole('button', { name: 'Databases & rigs' }));
    fireEvent.click(screen.getByRole('tab', { name: 'New / import' }));
    expect(await screen.findByRole('button', { name: /Import from Target Scheduler/ })).toBeEnabled();
    expect(screen.queryByText('Editing database')).not.toBeInTheDocument();
  });
});

describe('TauriSettings first run', () => {
  function serveEmptyRegistry() {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        })
      )
    );
  }

  it('offers importing images as well as opening an existing database', async () => {
    serveEmptyRegistry();

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(
      await screen.findByRole('button', { name: /New Database from Images/ })
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /Import from Target Scheduler/ })
    ).toBeInTheDocument();

    // Nothing that needs an existing catalog should sit between the welcome
    // banner and those two buttons.
    expect(
      screen.queryByText('Add a catalog before syncing with a remote PSF Guard.')
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        'Add a second catalog to merge data or send planning and grades.'
      )
    ).not.toBeInTheDocument();
  });

  it('reveals the Sync tab once a catalog is configured', async () => {
    server.use(
      http.get('/api/info', () =>
        HttpResponse.json({
          success: true,
          data: {
            version: 'test',
            cache_directory: '/tmp/cache',
            allow_database_management: true,
          },
          error: null,
          status: 'ready',
        })
      ),
      http.get('/api/databases', () =>
        HttpResponse.json({
          success: true,
          data: [
            {
              id: 'archive',
              name: 'Archive',
              database_path: '/tmp/archive.sqlite',
              image_directories: ['/images'],
            },
          ],
          error: null,
          status: 'ready',
        })
      )
    );

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Open Archive' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Database & files' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Sync' }));
    expect(
      await screen.findByRole('heading', { name: 'Remote PSF Guard' })
    ).toBeInTheDocument();
    // The database list belongs to its own tab, not this one.
    expect(
      screen.queryByRole('heading', { name: /Configured Databases/ })
    ).not.toBeInTheDocument();
  });

  it('opens straight onto the create form when the caller asks for it', async () => {
    serveEmptyRegistry();

    render(
      <TauriSettings isOpen initialIntent="create" onClose={() => undefined} />,
      { wrapper: createWrapper() }
    );

    expect(
      await screen.findByRole('heading', { name: 'New Database from Images' })
    ).toBeInTheDocument();
    // The create flow bootstraps its own database, so it never asks for one.
    expect(screen.queryByLabelText('Source database file:')).not.toBeInTheDocument();
  });

  it('keeps the catalog installer out of the way of the import form', async () => {
    serveEmptyRegistry();

    render(
      <TauriSettings isOpen initialIntent="create" onClose={() => undefined} />,
      { wrapper: createWrapper() }
    );

    // The form opens on the Databases tab with nothing above it. The catalog
    // installer used to render between the welcome banner and this form,
    // pushing it off the bottom of the modal.
    expect(
      await screen.findByRole('heading', { name: 'New Database from Images' })
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: 'Seiza Catalogs' })
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('tab', { name: 'Reference catalogs' }));
    expect(
      await screen.findByRole('heading', { name: 'Seiza Catalogs' })
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: 'New Database from Images' })
    ).not.toBeInTheDocument();
  });

  it('offers no Sync tab until a catalog exists', async () => {
    serveEmptyRegistry();

    render(<TauriSettings isOpen onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });

    expect(await screen.findByRole('tab', { name: 'Overview' })).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Reference catalogs' })).toBeInTheDocument();
    // Sync has nothing to talk about with no databases configured.
    expect(screen.queryByRole('tab', { name: 'Sync' })).not.toBeInTheDocument();
  });

  it('opens straight onto the add form when the caller asks for it', async () => {
    serveEmptyRegistry();

    render(
      <TauriSettings isOpen initialIntent="add" onClose={() => undefined} />,
      { wrapper: createWrapper() }
    );

    expect(
      await screen.findByRole('heading', { name: 'Import from Target Scheduler' })
    ).toBeInTheDocument();
    expect(screen.getByLabelText('Source database file:')).toBeInTheDocument();
  });

  it('keeps image-folder import available from the TS flow and preserves entered folders', async () => {
    serveEmptyRegistry();
    render(<TauriSettings isOpen initialIntent="add" onClose={() => undefined} />, { wrapper: createWrapper() });
    fireEvent.change(await screen.findByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/images' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Image folders' }));
    expect(screen.getByRole('heading', { name: 'New Database from Images' })).toBeInTheDocument();
    expect(screen.queryByPlaceholderText('Select or enter database path')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create & Import' })).toBeEnabled();
    expect(screen.getByText('📂 /images')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Configured Databases' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('radio', { name: 'Import a local TS file' }));
    expect(screen.getByPlaceholderText('Select or enter database path')).toBeInTheDocument();
    expect(screen.getByText('📂 /images')).toBeInTheDocument();
  });

  it('imports a standalone catalog copy at an advanced destination', async () => {
    serveEmptyRegistry();
    const requests: unknown[] = [];
    server.use(http.post('/api/databases/import-ts', async ({ request }) => {
      requests.push(await request.json());
      return HttpResponse.json({ success: true, data: { id: 'import:copy', catalog_id: null, name: 'Restored', running: true, automatic: false, source_connected: false, stage: 'copying', started_at: 1, pages_done: 0, pages_total: 0, error: null, report: null, stopping: false, waiting_for: null } });
    }));
    render(<TauriSettings isOpen initialIntent="add" onClose={() => undefined} />, { wrapper: createWrapper() });
    fireEvent.change(await screen.findByPlaceholderText('Select or enter database path'), { target: { value: '/backups/catalog.sqlite' } });
    fireEvent.change(screen.getByLabelText('Display name (optional):'), { target: { value: 'Restored' } });
    fireEvent.click(screen.getByLabelText('Keep the source connected for Target Scheduler sync'));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Use a custom database path (advanced)' }));
    fireEvent.change(screen.getByLabelText('Destination database path:'), { target: { value: '/srv/catalogs/restored.sqlite' } });
    fireEvent.change(screen.getByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/images' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    fireEvent.click(screen.getByRole('button', { name: 'Import database copy' }));
    await waitFor(() => expect(requests).toEqual([{ name: 'Restored', db_path: '/backups/catalog.sqlite', image_dirs: ['/images'], connect_source: false, destination_path: '/srv/catalogs/restored.sqlite' }]));
  });

  it.each([
    ['Image folders', 'Create & Import'],
    ['Use the PSF Guard Sync plugin', 'Create database for plugin'],
  ])('offers custom catalog storage for %s', async (source, save) => {
    serveEmptyRegistry();
    const requests: unknown[] = [];
    const database = { id: 'custom', name: 'Custom catalog', database_path: '/srv/custom.sqlite', image_directories: ['/images'] };
    server.use(http.post('/api/databases/create', async ({ request }) => {
      requests.push(await request.json());
      return HttpResponse.json({ success: true, data: { database, import: { stage: 'complete', running: false } } });
    }));
    render(<TauriSettings isOpen initialIntent="add" onClose={() => undefined} />, { wrapper: createWrapper() });
    fireEvent.click(await screen.findByRole('radio', { name: source }));
    expect(screen.getByRole('group', { name: 'Database storage' })).toBeVisible();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Use a custom database path (advanced)' }));
    fireEvent.change(screen.getByLabelText('Destination database path:'), { target: { value: '/srv/custom.sqlite' } });
    fireEvent.change(screen.getByLabelText('Display name (optional):'), { target: { value: 'Custom catalog' } });
    fireEvent.change(screen.getByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/images' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    fireEvent.click(screen.getByRole('button', { name: save }));
    await waitFor(() => expect(requests).toEqual([{ name: 'Custom catalog', image_dirs: ['/images'], backfill: false, fill_metadata: true, db_path: '/srv/custom.sqlite' }]));
  });

  it('validates a custom path and ignores it after managed storage is restored', async () => {
    serveEmptyRegistry();
    const requests: unknown[] = [];
    server.use(http.post('/api/databases/import-ts', async ({ request }) => {
      requests.push(await request.json());
      return HttpResponse.json({ success: true, data: { id: 'import:managed', running: true } });
    }));
    render(<TauriSettings isOpen initialIntent="add" onClose={() => undefined} />, { wrapper: createWrapper() });
    fireEvent.change(await screen.findByLabelText('Source database file:'), { target: { value: '/source.sqlite' } });
    fireEvent.change(screen.getByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/images' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    const custom = screen.getByRole('checkbox', { name: 'Use a custom database path (advanced)' });
    fireEvent.click(custom);
    fireEvent.change(screen.getByLabelText('Destination database path:'), { target: { value: 'relative.sqlite' } });
    fireEvent.click(screen.getByRole('button', { name: 'Import from Target Scheduler' }));
    expect(await screen.findByText('Enter an absolute destination database path, or turn off Use a custom database path (advanced).')).toBeVisible();
    expect(requests).toEqual([]);
    fireEvent.click(custom);
    expect(screen.queryByLabelText('Destination database path:')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Import from Target Scheduler' }));
    await waitFor(() => expect(requests).toEqual([{ name: 'source', db_path: '/source.sqlite', image_dirs: ['/images'] }]));
  });

  it('offers the Sync plugin and opens pairing after creating its catalog without a TS file', async () => {
    serveEmptyRegistry();
    const database = { id: 'plugin', name: 'Plugin catalog', database_path: '/managed/plugin.sqlite', image_directories: ['/receive'] };
    const requests: unknown[] = [];
    let created = false;
    server.use(
      http.get('/api/databases', () => HttpResponse.json({ success: true, data: created ? [database] : [] })),
      http.post('/api/databases/create', async ({ request }) => {
        requests.push(await request.json());
        created = true;
        return HttpResponse.json({ success: true, data: { database, import: { stage: 'complete', running: false } } });
      }),
      http.post('/api/databases/plugin/pairing-token', () => HttpResponse.json({ success: true, data: { pairing_token: 'ONE-USE-CODE', expires_at: 4102444800 } })),
    );
    render(<TauriSettings isOpen initialIntent="add" onClose={() => undefined} />, { wrapper: createWrapper() });
    fireEvent.click(await screen.findByLabelText('Use the PSF Guard Sync plugin'));
    expect(screen.queryByPlaceholderText('Select or enter database path')).not.toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText('e.g. Imaging Rig (defaults to filename)'), { target: { value: 'Plugin catalog' } });
    fireEvent.change(screen.getByPlaceholderText('Type an absolute path and press Add'), { target: { value: '/receive' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    fireEvent.click(screen.getByRole('button', { name: 'Create database for plugin' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Generate pairing code' }));
    expect(await screen.findByDisplayValue('ONE-USE-CODE')).toBeInTheDocument();
    expect(requests).toEqual([{ name: 'Plugin catalog', image_dirs: ['/receive'], backfill: false, fill_metadata: true }]);
    expect(screen.getByText('https://nina-plugins.psf-guard.com/')).toBeInTheDocument();
  });
});
