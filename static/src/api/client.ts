import axios from 'axios';
import { getServerUrl } from '../utils/tauri';
import type {
  ApiResponse,
  Project,
  Target,
  Image,
  ImageQuery,
  UpdateGradeRequest,
  StarDetectionResponse,
  PreviewOptions,
  ServerInfo,
  FileCheckResponse,
  DirectoryTreeResponse,
  ProjectOverview,
  TargetOverview,
  OverallStats,
  CacheRefreshProgress,
  SequenceAnalysisRequest,
  SequenceAnalysisResponse,
  ImageQualityResponse,
} from './types';

// Default API instance (used as fallback)
const api = axios.create({
  baseURL: '/api',
  headers: {
    'Content-Type': 'application/json',
  },
});

// Store the initialized API instance and server URL
let initializedApi: typeof api | null = null;
let cachedServerUrl: string | null = null;

// Initialize the API client
const initializeApi = async () => {
  if (!initializedApi) {
    const serverUrl = await getServerUrl();
    cachedServerUrl = serverUrl;
    
    const baseURL = serverUrl ? `${serverUrl}/api` : '/api';
    initializedApi = axios.create({
      baseURL,
      headers: {
        'Content-Type': 'application/json',
      },
    });
    
    // Add response interceptor for error handling
    initializedApi.interceptors.response.use(
      (response) => response,
      (error) => {
        console.error('API Error:', error);
        return Promise.reject(error);
      }
    );
  }
  return initializedApi;
};

// Get the API instance (initialize if needed)
const getApi = async () => {
  return await initializeApi();
};

// Get cached server URL for synchronous URL building
const getCachedServerUrl = (): string => {
  return cachedServerUrl || '';
};

// Initialize the client early - call this in your app root
export const initializeApiClient = async () => {
  await initializeApi();
};

export const apiClient = {
  // Server info
  getServerInfo: async (): Promise<ServerInfo> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ServerInfo>>('/info');
    if (!data.data) throw new Error('Failed to get server info');
    return data.data;
  },

  // Refresh file cache
  refreshFileCache: async (): Promise<FileCheckResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<FileCheckResponse>>('/refresh-cache');
    if (!data.data) throw new Error('Failed to refresh cache');
    return data.data;
  },

  // Refresh directory cache
  refreshDirectoryCache: async (): Promise<DirectoryTreeResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<DirectoryTreeResponse>>('/refresh-directory-cache');
    if (!data.data) throw new Error('Failed to refresh directory cache');
    return data.data;
  },

  // Projects
  getProjects: async (): Promise<Project[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Project[]>>('/projects');
    return data.data || [];
  },

  // Targets
  getTargets: async (projectId: number): Promise<Target[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Target[]>>(`/projects/${projectId}/targets`);
    return data.data || [];
  },

  // Images
  getImages: async (query: ImageQuery): Promise<Image[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Image[]>>('/images', { params: query });
    return data.data || [];
  },

  getImage: async (imageId: number): Promise<Image> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Image>>(`/images/${imageId}`);
    if (!data.data) throw new Error('Image not found');
    return data.data;
  },

  // Grading
  updateImageGrade: async (imageId: number, request: UpdateGradeRequest): Promise<void> => {
    const apiInstance = await getApi();
    await apiInstance.put(`/images/${imageId}/grade`, request);
  },

  // Star detection
  getStarDetection: async (imageId: number): Promise<StarDetectionResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<StarDetectionResponse>>(`/images/${imageId}/stars`);
    if (!data.data) throw new Error('Star detection failed');
    return data.data;
  },

  // Preview URL builder (uses cached server URL for synchronous access)
  getPreviewUrl: (imageId: number, options?: PreviewOptions): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    if (options?.size) params.append('size', options.size);
    if (options?.stretch !== undefined) params.append('stretch', String(options.stretch));
    if (options?.midtone !== undefined) params.append('midtone', String(options.midtone));
    if (options?.shadow !== undefined) params.append('shadow', String(options.shadow));
    
    const queryString = params.toString();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}/images/${imageId}/preview${queryString ? `?${queryString}` : ''}`;
  },

  // Annotated image URL (uses cached server URL for synchronous access)
  getAnnotatedUrl: (imageId: number, size: 'screen' | 'large' | 'original' = 'large', maxStars?: number): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    params.append('size', size);
    if (maxStars !== undefined) {
      params.append('max_stars', String(maxStars));
    }
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}/images/${imageId}/annotated?${params.toString()}`;
  },

  // PSF visualization URL (uses cached server URL for synchronous access)
  getPsfUrl: (imageId: number, options?: {
    num_stars?: number;
    psf_type?: string;
    sort_by?: string;
    grid_cols?: number;
    selection?: string;
  }): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    if (options?.num_stars) params.append('num_stars', String(options.num_stars));
    if (options?.psf_type) params.append('psf_type', options.psf_type);
    if (options?.sort_by) params.append('sort_by', options.sort_by);
    if (options?.grid_cols) params.append('grid_cols', String(options.grid_cols));
    if (options?.selection) params.append('selection', options.selection);
    
    const queryString = params.toString();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}/images/${imageId}/psf${queryString ? `?${queryString}` : ''}`;
  },

  // Overview endpoints
  getProjectsOverview: async (): Promise<ProjectOverview[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ProjectOverview[]>>('/projects/overview');
    return data.data || [];
  },

  getTargetsOverview: async (): Promise<TargetOverview[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<TargetOverview[]>>('/targets/overview');
    return data.data || [];
  },

  getOverallStats: async (): Promise<OverallStats> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<OverallStats>>('/stats/overall');
    if (!data.data) throw new Error('Failed to get overall stats');
    return data.data;
  },

  // Cache progress
  getCacheProgress: async (): Promise<CacheRefreshProgress> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<CacheRefreshProgress>>('/cache-progress');
    if (!data.data) throw new Error('Failed to get cache progress');
    return data.data;
  },

  // Sequence analysis
  analyzeSequence: async (request: SequenceAnalysisRequest): Promise<SequenceAnalysisResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<SequenceAnalysisResponse>>('/analysis/sequence', { params: request });
    if (!data.data) throw new Error('Sequence analysis failed');
    return data.data;
  },

  getImageQuality: async (imageId: number): Promise<ImageQualityResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ImageQualityResponse>>(`/analysis/image/${imageId}`);
    if (!data.data) throw new Error('Image quality data not found');
    return data.data;
  },
};