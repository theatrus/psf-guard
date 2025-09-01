import axios from 'axios';
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
  ProjectOverview,
  TargetOverview,
  OverallStats,
} from './types';

const api = axios.create({
  baseURL: '/api',
  headers: {
    'Content-Type': 'application/json',
  },
});

// Add response interceptor for error handling
api.interceptors.response.use(
  (response) => response,
  (error) => {
    console.error('API Error:', error);
    return Promise.reject(error);
  }
);

export const apiClient = {
  // Server info
  getServerInfo: async (): Promise<ServerInfo> => {
    const { data } = await api.get<ApiResponse<ServerInfo>>('/info');
    if (!data.data) throw new Error('Failed to get server info');
    return data.data;
  },

  // Refresh file cache
  refreshFileCache: async (): Promise<FileCheckResponse> => {
    const { data } = await api.put<ApiResponse<FileCheckResponse>>('/refresh-cache');
    if (!data.data) throw new Error('Failed to refresh cache');
    return data.data;
  },

  // Projects
  getProjects: async (): Promise<Project[]> => {
    const { data } = await api.get<ApiResponse<Project[]>>('/projects');
    return data.data || [];
  },

  // Targets
  getTargets: async (projectId: number): Promise<Target[]> => {
    const { data } = await api.get<ApiResponse<Target[]>>(`/projects/${projectId}/targets`);
    return data.data || [];
  },

  // Images
  getImages: async (query: ImageQuery): Promise<Image[]> => {
    const { data } = await api.get<ApiResponse<Image[]>>('/images', { params: query });
    return data.data || [];
  },

  getImage: async (imageId: number): Promise<Image> => {
    const { data } = await api.get<ApiResponse<Image>>(`/images/${imageId}`);
    if (!data.data) throw new Error('Image not found');
    return data.data;
  },

  // Grading
  updateImageGrade: async (imageId: number, request: UpdateGradeRequest): Promise<void> => {
    await api.put(`/images/${imageId}/grade`, request);
  },

  // Star detection
  getStarDetection: async (imageId: number): Promise<StarDetectionResponse> => {
    const { data } = await api.get<ApiResponse<StarDetectionResponse>>(`/images/${imageId}/stars`);
    if (!data.data) throw new Error('Star detection failed');
    return data.data;
  },

  // Preview URL builder (doesn't make a request, just returns the URL)
  getPreviewUrl: (imageId: number, options?: PreviewOptions): string => {
    const params = new URLSearchParams();
    if (options?.size) params.append('size', options.size);
    if (options?.stretch !== undefined) params.append('stretch', String(options.stretch));
    if (options?.midtone !== undefined) params.append('midtone', String(options.midtone));
    if (options?.shadow !== undefined) params.append('shadow', String(options.shadow));
    
    const queryString = params.toString();
    return `/api/images/${imageId}/preview${queryString ? `?${queryString}` : ''}`;
  },

  // Annotated image URL
  getAnnotatedUrl: (imageId: number, size: 'screen' | 'large' | 'original' = 'large', maxStars?: number): string => {
    const params = new URLSearchParams();
    params.append('size', size);
    if (maxStars !== undefined) {
      params.append('max_stars', String(maxStars));
    }
    return `/api/images/${imageId}/annotated?${params.toString()}`;
  },

  // PSF visualization URL
  getPsfUrl: (imageId: number, options?: {
    num_stars?: number;
    psf_type?: string;
    sort_by?: string;
    grid_cols?: number;
    selection?: string;
  }): string => {
    const params = new URLSearchParams();
    if (options?.num_stars) params.append('num_stars', String(options.num_stars));
    if (options?.psf_type) params.append('psf_type', options.psf_type);
    if (options?.sort_by) params.append('sort_by', options.sort_by);
    if (options?.grid_cols) params.append('grid_cols', String(options.grid_cols));
    if (options?.selection) params.append('selection', options.selection);
    
    const queryString = params.toString();
    return `/api/images/${imageId}/psf${queryString ? `?${queryString}` : ''}`;
  },

  // Overview endpoints
  getProjectsOverview: async (): Promise<ProjectOverview[]> => {
    const { data } = await api.get<ApiResponse<ProjectOverview[]>>('/projects/overview');
    return data.data || [];
  },

  getTargetsOverview: async (): Promise<TargetOverview[]> => {
    const { data } = await api.get<ApiResponse<TargetOverview[]>>('/targets/overview');
    return data.data || [];
  },

  getOverallStats: async (): Promise<OverallStats> => {
    const { data } = await api.get<ApiResponse<OverallStats>>('/stats/overall');
    if (!data.data) throw new Error('Failed to get overall stats');
    return data.data;
  },
};