import axios from 'axios';
import type { AxiosInstance } from 'axios';
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
  DatabaseSummary,
  FileCheckResponse,
  DirectoryTreeResponse,
  ProjectOverview,
  TargetOverview,
  OverallStats,
  CacheRefreshProgress,
  SequenceAnalysisRequest,
  SequenceAnalysisResponse,
  ImageQualityResponse,
  SpatialScanRequest,
  SpatialScanStatus,
  PreviewDescriptor,
  GenerationStatus,
  AstrometryAnalysis,
  SatelliteAnalysis,
  SatelliteAnalysisStatus,
  StackPreviewJob,
  LatestStackPreviews,
} from './types';

// Store the initialized API instance and server URL
let initializedApi: AxiosInstance | null = null;
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

// Build a per-DB path under /api/db/{dbId}.
const dbPath = (dbId: string, path: string) => `/db/${encodeURIComponent(dbId)}${path}`;

export const apiClient = {
  // ── Global ────────────────────────────────────────────────────────────────

  getServerInfo: async (): Promise<ServerInfo> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ServerInfo>>('/info');
    if (!data.data) throw new Error('Failed to get server info');
    return data.data;
  },

  /** List every configured database. */
  getDatabases: async (): Promise<DatabaseSummary[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<DatabaseSummary[]>>('/databases');
    return data.data || [];
  },

  /** Register a new database. Works in both Tauri and browser mode. */
  addDatabase: async (req: {
    name: string;
    db_path: string;
    image_dirs: string[];
    slug?: string;
  }): Promise<DatabaseSummary> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<DatabaseSummary>>('/databases', req);
    if (!data.data) throw new Error(data.error || 'Failed to add database');
    return data.data;
  },

  /** Update an existing database. */
  updateDatabase: async (
    dbId: string,
    req: {
      name?: string;
      slug?: string;
      db_path?: string;
      image_dirs?: string[];
    }
  ): Promise<DatabaseSummary> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<DatabaseSummary>>(
      `/databases/${encodeURIComponent(dbId)}`,
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to update database');
    return data.data;
  },

  /** Remove a database by slug. */
  removeDatabase: async (dbId: string): Promise<boolean> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.delete<ApiResponse<{ removed: boolean }>>(
      `/databases/${encodeURIComponent(dbId)}`
    );
    return data.data?.removed ?? false;
  },

  // ── Per-DB ────────────────────────────────────────────────────────────────

  refreshFileCache: async (dbId: string): Promise<FileCheckResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<FileCheckResponse>>(
      dbPath(dbId, '/refresh-cache')
    );
    if (!data.data) throw new Error('Failed to refresh cache');
    return data.data;
  },

  refreshDirectoryCache: async (dbId: string): Promise<DirectoryTreeResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<DirectoryTreeResponse>>(
      dbPath(dbId, '/refresh-directory-cache')
    );
    if (!data.data) throw new Error('Failed to refresh directory cache');
    return data.data;
  },

  getProjects: async (dbId: string): Promise<Project[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Project[]>>(dbPath(dbId, '/projects'));
    return data.data || [];
  },

  getTargets: async (dbId: string, projectId: number): Promise<Target[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Target[]>>(
      dbPath(dbId, `/projects/${projectId}/targets`)
    );
    return data.data || [];
  },

  startStackPreviews: async (
    dbId: string,
    projectId: number,
    request: { image_ids: number[]; accepted_only: boolean; force?: boolean }
  ): Promise<StackPreviewJob> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<StackPreviewJob>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews`),
      request
    );
    if (!data.data) throw new Error(data.error || 'Failed to start stack previews');
    return data.data;
  },

  getStackPreviewJob: async (
    dbId: string,
    projectId: number,
    jobId: string
  ): Promise<StackPreviewJob> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<StackPreviewJob>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews/${encodeURIComponent(jobId)}`)
    );
    if (!data.data) throw new Error(data.error || 'Stack preview job not found');
    return data.data;
  },

  getLatestStackPreviews: async (
    dbId: string,
    projectId: number
  ): Promise<LatestStackPreviews> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<LatestStackPreviews>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews/latest`)
    );
    if (!data.data) throw new Error(data.error || 'Latest stack previews could not be loaded');
    return data.data;
  },

  getStackPreviewUrl: (
    dbId: string,
    jobId: string,
    groupIndex: number,
    artifactRevision: string,
    size: 'screen' | 'original' = 'screen'
  ): string => {
    const serverUrl = getCachedServerUrl();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    const params = new URLSearchParams();
    if (size === 'original') params.set('size', size);
    if (artifactRevision) params.set('v', artifactRevision);
    const query = params.size ? `?${params.toString()}` : '';
    return `${basePath}${dbPath(
      dbId,
      `/stack-previews/${encodeURIComponent(jobId)}/${groupIndex}/preview`
    )}${query}`;
  },

  getStackFitsUrl: (
    dbId: string,
    jobId: string,
    groupIndex: number,
    artifactRevision: string
  ): string => {
    const serverUrl = getCachedServerUrl();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    const revision = artifactRevision ? `?v=${encodeURIComponent(artifactRevision)}` : '';
    return `${basePath}${dbPath(
      dbId,
      `/stack-previews/${encodeURIComponent(jobId)}/${groupIndex}/fits`
    )}${revision}`;
  },

  getImages: async (dbId: string, query: ImageQuery): Promise<Image[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Image[]>>(dbPath(dbId, '/images'), {
      params: query,
    });
    return data.data || [];
  },

  getImage: async (dbId: string, imageId: number): Promise<Image> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<Image>>(
      dbPath(dbId, `/images/${imageId}`)
    );
    if (!data.data) throw new Error('Image not found');
    return data.data;
  },

  getImageAstrometry: async (dbId: string, imageId: number): Promise<AstrometryAnalysis> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<AstrometryAnalysis>>(
      dbPath(dbId, `/images/${imageId}/astrometry`)
    );
    if (!data.data) throw new Error(data.error || 'Astrometry analysis unavailable');
    return data.data;
  },

  solveImageAstrometry: async (dbId: string, imageId: number): Promise<AstrometryAnalysis> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<AstrometryAnalysis>>(
      dbPath(dbId, `/images/${imageId}/astrometry`)
    );
    if (!data.data) throw new Error(data.error || 'Plate solve failed');
    return data.data;
  },

  getImageSatellites: async (dbId: string, imageId: number): Promise<SatelliteAnalysisStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<SatelliteAnalysisStatus>>(
      dbPath(dbId, `/images/${imageId}/satellites`)
    );
    if (!data.data) throw new Error(data.error || 'Satellite analysis unavailable');
    return data.data;
  },

  predictImageSatellites: async (dbId: string, imageId: number): Promise<SatelliteAnalysis> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<SatelliteAnalysis>>(
      dbPath(dbId, `/images/${imageId}/satellites`)
    );
    if (!data.data) throw new Error(data.error || 'Satellite prediction failed');
    return data.data;
  },

  updateImageGrade: async (
    dbId: string,
    imageId: number,
    request: UpdateGradeRequest
  ): Promise<void> => {
    const apiInstance = await getApi();
    await apiInstance.put(dbPath(dbId, `/images/${imageId}/grade`), request);
  },

  getStarDetection: async (dbId: string, imageId: number): Promise<StarDetectionResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<StarDetectionResponse>>(
      dbPath(dbId, `/images/${imageId}/stars`)
    );
    if (!data.data) throw new Error('Star detection failed');
    return data.data;
  },

  // Batch readiness poll for on-demand previews/annotated images. One request
  // for a whole grid of pending images instead of one poll per image. Returns
  // statuses parallel to `requests`.
  getGenerationStatus: async (
    dbId: string,
    requests: PreviewDescriptor[]
  ): Promise<GenerationStatus[]> => {
    const apiInstance = await getApi();
    const body = {
      requests: requests.map((d) => ({
        image_id: d.imageId,
        kind: d.kind,
        size: d.size,
        stretch: d.stretch,
        midtone: d.midtone,
        shadow: d.shadow,
        max_stars: d.maxStars,
      })),
    };
    const { data } = await apiInstance.post<
      ApiResponse<{ statuses: GenerationStatus[] }>
    >(dbPath(dbId, '/images/generation-status'), body);
    return data.data?.statuses ?? [];
  },

  getPreviewUrl: (dbId: string, imageId: number, options?: PreviewOptions): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    if (options?.size) params.append('size', options.size);
    if (options?.stretch !== undefined) params.append('stretch', String(options.stretch));
    if (options?.midtone !== undefined) params.append('midtone', String(options.midtone));
    if (options?.shadow !== undefined) params.append('shadow', String(options.shadow));

    const queryString = params.toString();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}${dbPath(dbId, `/images/${imageId}/preview`)}${
      queryString ? `?${queryString}` : ''
    }`;
  },

  getAnnotatedUrl: (
    dbId: string,
    imageId: number,
    size: 'screen' | 'large' | 'original' = 'large',
    maxStars?: number
  ): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    params.append('size', size);
    if (maxStars !== undefined) {
      params.append('max_stars', String(maxStars));
    }
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}${dbPath(dbId, `/images/${imageId}/annotated`)}?${params.toString()}`;
  },

  getPsfUrl: (
    dbId: string,
    imageId: number,
    options?: {
      num_stars?: number;
      psf_type?: string;
      sort_by?: string;
      grid_cols?: number;
      selection?: string;
    }
  ): string => {
    const serverUrl = getCachedServerUrl();
    const params = new URLSearchParams();
    if (options?.num_stars) params.append('num_stars', String(options.num_stars));
    if (options?.psf_type) params.append('psf_type', options.psf_type);
    if (options?.sort_by) params.append('sort_by', options.sort_by);
    if (options?.grid_cols) params.append('grid_cols', String(options.grid_cols));
    if (options?.selection) params.append('selection', options.selection);

    const queryString = params.toString();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    return `${basePath}${dbPath(dbId, `/images/${imageId}/psf`)}${
      queryString ? `?${queryString}` : ''
    }`;
  },

  getProjectsOverview: async (dbId: string): Promise<ProjectOverview[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ProjectOverview[]>>(
      dbPath(dbId, '/projects/overview')
    );
    return data.data || [];
  },

  getTargetsOverview: async (dbId: string): Promise<TargetOverview[]> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<TargetOverview[]>>(
      dbPath(dbId, '/targets/overview')
    );
    return data.data || [];
  },

  getOverallStats: async (dbId: string): Promise<OverallStats> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<OverallStats>>(
      dbPath(dbId, '/stats/overall')
    );
    if (!data.data) throw new Error('Failed to get overall stats');
    return data.data;
  },

  getCacheProgress: async (dbId: string): Promise<CacheRefreshProgress> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<CacheRefreshProgress>>(
      dbPath(dbId, '/cache-progress')
    );
    if (!data.data) throw new Error('Failed to get cache progress');
    return data.data;
  },

  analyzeSequence: async (
    dbId: string,
    request: SequenceAnalysisRequest
  ): Promise<SequenceAnalysisResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<SequenceAnalysisResponse>>(
      dbPath(dbId, '/analysis/sequence'),
      { params: request }
    );
    if (!data.data) throw new Error('Sequence analysis failed');
    return data.data;
  },

  getImageQuality: async (dbId: string, imageId: number): Promise<ImageQualityResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ImageQualityResponse>>(
      dbPath(dbId, `/analysis/image/${imageId}`)
    );
    if (!data.data) throw new Error('Image quality data not found');
    return data.data;
  },

  startSpatialScan: async (
    dbId: string,
    request: SpatialScanRequest
  ): Promise<SpatialScanStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<SpatialScanStatus>>(
      dbPath(dbId, '/analysis/quality-scan'),
      request
    );
    if (!data.data) throw new Error('Failed to start spatial scan');
    return data.data;
  },

  getSpatialScanStatus: async (dbId: string): Promise<SpatialScanStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<SpatialScanStatus>>(
      dbPath(dbId, '/analysis/quality-scan')
    );
    if (!data.data) throw new Error('Failed to get spatial scan status');
    return data.data;
  },
};
