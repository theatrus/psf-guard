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
  
  // For dev mode, assume Tauri if we're on localhost with Vite's typical ports
  const isViteDevMode = window.location.hostname === 'localhost' && 
                       (window.location.port === '5173' || window.location.port === '5174');

  // Check if we're in a Tauri context (production or development)
  return hasTauri || hasTauriApi || hasInvoke || isWebview || isViteDevMode;
};

// Get the server URL when running in Tauri mode
export const getServerUrl = async (): Promise<string> => {
  if (isTauriApp()) {
    try {
      // @ts-ignore - Tauri API will be available at runtime
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

// Configuration interface matching Rust struct
export interface TauriConfig {
  database_path?: string | null;
  image_directories: string[];
}

// Tauri-specific file system functions
export const tauriFileSystem = {
  // Pick database file using Tauri command
  pickDatabaseFile: async (): Promise<string | null> => {
    if (!isTauriApp()) return null;
    
    try {
      // @ts-ignore - Tauri API will be available at runtime
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
      // @ts-ignore - Tauri API will be available at runtime
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
      // @ts-ignore - Tauri API will be available at runtime
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
  // Get current configuration
  getCurrentConfiguration: async (): Promise<TauriConfig | null> => {
    if (!isTauriApp()) return null;
    
    try {
      // @ts-ignore - Tauri API will be available at runtime
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('get_current_configuration');
    } catch (error) {
      console.error('Failed to get current configuration:', error);
      return null;
    }
  },

  // Save configuration
  saveConfiguration: async (config: TauriConfig): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      // @ts-ignore - Tauri API will be available at runtime
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('save_configuration', { config });
      return true;
    } catch (error) {
      console.error('Failed to save configuration:', error);
      return false;
    }
  },

  // Restart application to apply new configuration
  restartApplication: async (): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      // @ts-ignore - Tauri API will be available at runtime
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('restart_application');
      return true;
    } catch (error) {
      console.error('Failed to restart application:', error);
      return false;
    }
  },

  // Check if current configuration is valid
  isConfigurationValid: async (): Promise<boolean> => {
    if (!isTauriApp()) return false;
    
    try {
      // @ts-ignore - Tauri API will be available at runtime
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke('is_configuration_valid');
    } catch (error) {
      console.error('Failed to check configuration validity:', error);
      return false;
    }
  }
};