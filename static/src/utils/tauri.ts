// Utility functions for Tauri integration (updated with enhanced detection)

// Check if we're running in Tauri
export const isTauriApp = (): boolean => {
  if (typeof window === 'undefined') return false;
  
  // Check for various Tauri globals that might be available
  const hasTauri = '__TAURI__' in window;
  const hasTauriApi = '__TAURI_INTERNALS__' in window;
  const hasInvoke = 'invoke' in window;
  
  // In development mode, check if we're running in a webview with specific characteristics
  const isWebview = window.navigator.userAgent.includes('Tauri') || 
                   window.location.protocol === 'tauri:' ||
                   window.location.hostname === 'tauri.localhost';

  // Check if we're in a Tauri context (production or development)
  // Only trust the presence of actual Tauri APIs, not URL patterns
  return hasTauri || hasTauriApi || hasInvoke || isWebview;
};

/**
 * Call a desktop command, or settle for `fallback` outside the desktop app or
 * when an optional command fails.
 */
async function invokeOr<T>(command: string, fallback: T, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriApp()) return fallback;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    return await invoke<T>(command, args);
  } catch (error) {
    console.error(`Desktop command ${command} failed:`, error);
    return fallback;
  }
}

/** What `invokeOr` returns for a command that did not run. */
const FAILED = Symbol('failed');

// The server URL in the desktop app; relative URLs (current origin) in a browser.
export const getServerUrl = async (): Promise<string> =>
  isTauriApp() ? desktopInvoke<string>('get_server_url') : '';

// Server failures must reach the UI. A guessed URL could belong to another
// process when the chosen port is occupied.
async function desktopInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    return await invoke<T>(command, args);
  } catch (error) {
    throw error instanceof Error ? error : new Error(String(error));
  }
}

export interface DesktopServerStatus {
  server_port: number | null;
  active_server_port: number | null;
  server_url: string | null;
  starting: boolean;
  error: string | null;
}

export const desktopServer = {
  getSettings: (): Promise<DesktopServerStatus> => desktopInvoke('get_desktop_server_settings'),
  savePort: (port: number | null): Promise<DesktopServerStatus> =>
    desktopInvoke('set_desktop_server_port', { port }),
};

// Initialize the base URL for API calls
export const initializeApiBaseUrl = async (): Promise<string> => {
  const serverUrl = await getServerUrl();
  return serverUrl ? `${serverUrl}/api` : '/api';
};

// Per-DB overrides for the reject-archive feature (mirrors
// `RejectArchiveOverrides` in src/db_registry.rs). All fields optional;
// missing keys fall through to the CLI flag, then the compiled-in defaults
// (`REJECT`, depth 1, `.json` / `.txt`).
export interface RejectArchiveOverrides {
  segment_name?: string;
  depth?: number;
  sidecar_exts?: string[];
}

// One configured database entry (mirrors `DbEntry` in the Rust db_registry module).
export interface DbEntry {
  ts_sync?: import('../api/tsSyncTypes').TsSyncSettings;
  id: string;
  name: string;
  db_path: string;
  image_dirs: string[];
  reject_archive?: RejectArchiveOverrides;
  remote_image_upload?: {
    enabled: boolean;
    image_dir?: string;
    token_sha256?: string;
    token_configured?: boolean;
    sync_enabled?: boolean;
    placement?: 'flat' | 'target_tree';
    directory_template?: string;
    catalog_directory_template?: string;
    directory_template_source?: 'catalog' | 'preset';
    directory_template_samples?: number;
    /** Paired clients; each holds its own revocable credential. */
    clients?: { client_uuid: string; name: string; paired_at: number }[];
  };
  /** Server-side destination for UI-triggered exports. */
  export_dir?: string;
  process_dir?: string;
  /** Automatic import of new frames, when turned on. */
  autoimport?: import('../api/types').AutoImportSettings;
  /** What every import from the image folders leaves out. */
  scan_rules?: import('../api/types').ScanRules;
}

// Process-global Seiza catalog paths. data_dir configures a complete bundle;
// relative overrides resolve below it. Without either, Seiza searches its
// standard environment, executable-adjacent, and platform data locations.
export interface AstrometryConfig {
  data_dir?: string;
  objects?: string;
  stars?: string;
  star_identifiers?: string;
  blind_index?: string;
  transients?: string;
  minor_bodies?: string;
  satellite_elements?: string;
}

// Persisted registry of all configured databases (mirrors `DbRegistry`).
export interface DbRegistry {
  schema_version: number;
  databases: DbEntry[];
  active_db_id?: string | null;
  astrometry?: AstrometryConfig;
}

// Backwards-compat alias; existing call sites referenced `TauriConfig`.
// Now points at the multi-DB registry shape.
export type TauriConfig = DbRegistry;

// Desktop file dialogs and the file manager.
export const tauriFileSystem = {
  pickDatabaseFile: (): Promise<string | null> => invokeOr('pick_database_file', null),

  /** A folder, or a file with `file`, for any path setting. */
  pickPath: (options: { title?: string; file?: boolean } = {}): Promise<string | null> =>
    invokeOr('pick_folder', null, { title: options.title, file: options.file ?? false }),

  // Show a resolved image file in Finder, Explorer, or the Linux file manager.
  showImageInFolder: async (dbId: string, path: string): Promise<void> => {
    if (!isTauriApp()) {
      throw new Error('Showing files is available only in the desktop app.');
    }
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('show_image_in_folder', { dbId, path });
  },

  // Default N.I.N.A. database path (Windows only).
  getDefaultNinaPath: (): Promise<string | null> => invokeOr('get_default_nina_database_path', null),

  /** The desktop app's log folder, or null outside it. */
  getLogFolder: (): Promise<string | null> => invokeOr('get_log_folder', null),

  /** Open the log folder in Finder, Explorer, or the Linux file manager. */
  openLogFolder: async (): Promise<void> => {
    if (!isTauriApp()) {
      throw new Error('Logs are written by the desktop app only.');
    }
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('open_log_folder');
  },
};

// The registry and the desktop app's own server.
export const tauriConfig = {
  getCurrentConfiguration: (): Promise<DbRegistry | null> =>
    invokeOr('get_current_configuration', null),

  restartApplication: async (): Promise<boolean> =>
    (await invokeOr<unknown>('restart_application', FAILED)) !== FAILED,

  // Restart only the server (faster than the whole app; moves no files).
  restartServer: (): Promise<string> => desktopInvoke('restart_server'),

  isConfigurationValid: (): Promise<boolean> => invokeOr('is_configuration_valid', false),
};

/** One server folder and where this computer sees it, for shared files. */
export interface WorkerPathMap {
  server: string;
  local: string;
}

/** The worker this computer runs: its settings, without the token. */
export interface DesktopWorkerSettings {
  server: string;
  name: string | null;
  files: 'remote' | 'shared';
  maps: WorkerPathMap[];
  work_dir: string | null;
  rc_astro: string | null;
  /** `--work` choices; empty takes everything the worker can run. */
  work: string[];
  slots: number;
}

/** What the running worker is doing (mirrors `WorkerStatus`). */
export interface DesktopWorkerStatus {
  server: string;
  name: string;
  version: string;
  connection: 'connecting' | 'connected' | 'failed' | 'stopped';
  paused: boolean;
  tasks: Array<{ id: string; label: string; fraction: number }>;
  problem: string | null;
}

export interface DesktopWorker {
  /** Whether this system runs the worker in the menu bar or the tray. */
  supported: boolean;
  settings: DesktopWorkerSettings | null;
  has_token: boolean;
  start_at_login: boolean;
  /** `null` while the worker is not running. */
  status: DesktopWorkerStatus | null;
  /** The Windows service's state, when the service is installed. */
  service: 'running' | 'stopped' | 'changing' | null;
  settings_path: string;
}

export interface SaveDesktopWorker extends DesktopWorkerSettings {
  /** Blank keeps the saved token. */
  token: string | null;
  start_at_login: boolean;
}

// The worker this computer runs in the menu bar or the system tray.
export const desktopWorker = {
  get: (): Promise<DesktopWorker> => desktopInvoke('get_desktop_worker'),
  save: (worker: SaveDesktopWorker): Promise<DesktopWorker> =>
    desktopInvoke('save_desktop_worker', { worker }),
  start: (): Promise<DesktopWorker> => desktopInvoke('start_desktop_worker'),
  stop: (): Promise<DesktopWorker> => desktopInvoke('stop_desktop_worker'),
  pause: (paused: boolean): Promise<DesktopWorker> => desktopInvoke('pause_desktop_worker', { paused }),
};
