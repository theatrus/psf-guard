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

// Get the server URL when running in Tauri mode
export const getServerUrl = async (): Promise<string> => {
  if (isTauriApp()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('get_server_url');
    } catch (error) {
      console.error('Failed to get server URL from Tauri:', error);
      // Fallback to default
      return 'http://localhost:3030';
    }
  }
  
  // In web mode, use relative URLs (current origin)
  return '';
};

// Initialize the base URL for API calls
export const initializeApiBaseUrl = async (): Promise<string> => {
  const serverUrl = await getServerUrl();
  return serverUrl ? `${serverUrl}/api` : '/api';
};

// Per-DB overrides for the reject-archive feature (mirrors
// `RejectArchiveOverrides` in src/db_registry.rs). All fields optional;
// missing keys fall through to the CLI flag, then the compiled-in defaults
// (`REJECT`, depth 1, `.xisf` / `.json` / `.txt`).
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
}

// Process-global Seiza catalog paths. Relative filenames resolve below
// data_dir; omitted filenames use Seiza's canonical bundle names.
export interface AstrometryConfig {
  data_dir?: string;
  objects?: string;
  stars?: string;
  star_identifiers?: string;
  blind_index?: string;
  transients?: string;
  minor_bodies?: string;
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

// Tauri-specific file system functions
export const tauriFileSystem = {
  // Pick database file using Tauri command
  pickDatabaseFile: async (): Promise<string | null> => {
    if (!isTauriApp()) return null;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('pick_database_file');
    } catch (error) {
      console.error('Failed to pick database file:', error);
      return null;
    }
  },

  // Pick image directory using Tauri command  
  pickImageDirectory: async (): Promise<string | null> => {
    if (!isTauriApp()) return null;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('pick_image_directory');
    } catch (error) {
      console.error('Failed to pick image directory:', error);
      return null;
    }
  },

  // Get default N.I.N.A. database path (Windows only)
  getDefaultNinaPath: async (): Promise<string | null> => {
    if (!isTauriApp()) return null;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('get_default_nina_database_path');
    } catch (error) {
      console.error('Failed to get default N.I.N.A. path:', error);
      return null;
    }
  }
};

// Configuration management functions
export const tauriConfig = {
  // Get current configuration (registry of all configured DBs)
  getCurrentConfiguration: async (): Promise<DbRegistry | null> => {
    if (!isTauriApp()) return null;

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('get_current_configuration');
    } catch (error) {
      console.error('Failed to get current configuration:', error);
      return null;
    }
  },

  // Replace the entire registry. Used by the multi-DB settings panel.
  saveConfiguration: async (config: DbRegistry): Promise<boolean> => {
    if (!isTauriApp()) return false;

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('save_configuration', { config });
      return true;
    } catch (error) {
      console.error('Failed to save configuration:', error);
      return false;
    }
  },

  // Add a single database to the registry; the backend persists and returns the entry.
  addDatabase: async (
    name: string,
    dbPath: string,
    imageDirs: string[]
  ): Promise<DbEntry | null> => {
    if (!isTauriApp()) return null;

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('add_database', { name, dbPath, imageDirs });
    } catch (error) {
      console.error('Failed to add database:', error);
      return null;
    }
  },

  // Remove a database from the registry by slug.
  removeDatabase: async (dbId: string): Promise<boolean> => {
    if (!isTauriApp()) return false;

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('remove_database', { dbId });
    } catch (error) {
      console.error('Failed to remove database:', error);
      return false;
    }
  },

  // Restart application to apply new configuration
  restartApplication: async (): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('restart_application');
      return true;
    } catch (error) {
      console.error('Failed to restart application:', error);
      return false;
    }
  },

  // Restart server with new configuration (faster than full app restart)
  restartServer: async (): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const result = await invoke('restart_server');
      console.log('Server restart result:', result);
      return true;
    } catch (error) {
      console.error('Failed to restart server:', error);
      return false;
    }
  },

  // Check if current configuration is valid
  isConfigurationValid: async (): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('is_configuration_valid');
    } catch (error) {
      console.error('Failed to check configuration validity:', error);
      return false;
    }
  }
};
