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
 * when the command fails. Every helper below goes through here.
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
  isTauriApp() ? invokeOr('get_server_url', 'http://localhost:3030') : '';

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
};

// The registry and the desktop app's own server.
export const tauriConfig = {
  getCurrentConfiguration: (): Promise<DbRegistry | null> =>
    invokeOr('get_current_configuration', null),

  restartApplication: async (): Promise<boolean> =>
    (await invokeOr<unknown>('restart_application', FAILED)) !== FAILED,

  // Restart only the server (faster than the whole app; moves no files).
  restartServer: async (): Promise<boolean> =>
    (await invokeOr<unknown>('restart_server', FAILED)) !== FAILED,

  isConfigurationValid: (): Promise<boolean> => invokeOr('is_configuration_valid', false),
};
