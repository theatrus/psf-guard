// Utility functions for Tauri integration

// Check if we're running in Tauri
export const isTauriApp = (): boolean => {
  return typeof window !== 'undefined' && '__TAURI__' in window;
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