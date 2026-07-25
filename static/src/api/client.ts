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
  SchedulerSyncRequest,
  SchedulerSyncPreviewResponse,
  SchedulerSyncResponse,
  DatabaseSummary,
  CreateDatabaseRequest,
  CreateDatabaseResponse,
  ImportRequest,
  ImportStatus,
  FileCheckResponse,
  DirectoryTreeResponse,
  ProjectOverview,
  TargetOverview,
  ProjectSchedulerDetails,
  ProjectSchedulerUpdate,
  TargetSchedulerUpdate,
  CreateExposurePlanRequest,
  ExposurePlanDetails,
  OverallStats,
  CacheRefreshProgress,
  SequenceAnalysisRequest,
  SequenceAnalysisResponse,
  ImageQualityResponse,
  SpatialScanRequest,
  SpatialScanStatus,
  QualityBackfillRequest,
  QualityBackfillStatus,
  PreviewDescriptor,
  GenerationStatus,
  AstrometryAnalysis,
  SatelliteAnalysis,
  SatelliteAnalysisStatus,
  StackPreviewJob,
  LatestStackPreviews,
  StackColorCatalog,
  StackColorJob,
  StackColorKind,
  StackColorProcessing,
  StackNarrowbandPalette,
  StackStretchPreview,
  StackViewProcessingRequest,
  AstrometryCapabilities,
  AstrometryValidationReport,
  CatalogInstallPreset,
  CatalogInstallStatus,
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
        if (axios.isAxiosError<ApiResponse<unknown>>(error)) {
          const message = error.response?.data?.error;
          if (message) return Promise.reject(new Error(message, { cause: error }));
        }
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

const withServerUrl = (path: string): string => `${getCachedServerUrl()}${path}`;

const normalizeStretchPreview = (preview: StackStretchPreview): StackStretchPreview => ({
  ...preview,
  deconvolution_version: preview.deconvolution_version ?? null,
  deconvolution_id: preview.deconvolution_id ?? null,
  deconvolution: preview.deconvolution ?? null,
  preview_url: withServerUrl(preview.preview_url),
  original_preview_url: withServerUrl(preview.original_preview_url),
  fits_url: preview.fits_url ? withServerUrl(preview.fits_url) : null,
});

const stackStretchError = (cause: unknown, fallback: string): Error => {
  if (axios.isAxiosError<ApiResponse<unknown>>(cause)) {
    return new Error(cause.response?.data?.error || cause.message || fallback);
  }
  return cause instanceof Error ? cause : new Error(fallback);
};

export const apiClient = {
  // ── Global ────────────────────────────────────────────────────────────────

  getServerInfo: async (): Promise<ServerInfo> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ServerInfo>>('/info');
    if (!data.data) throw new Error('Failed to get server info');
    return data.data;
  },

  getAstrometryCapabilities: async (): Promise<AstrometryCapabilities> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<AstrometryCapabilities>>(
      '/astrometry/capabilities'
    );
    if (!data.data) throw new Error(data.error || 'Failed to inspect Seiza catalogs');
    return data.data;
  },

  validateAstrometryCatalogs: async (): Promise<AstrometryValidationReport> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<AstrometryValidationReport>>(
      '/astrometry/catalogs/validate'
    );
    if (!data.data) throw new Error(data.error || 'Failed to validate Seiza catalogs');
    return data.data;
  },

  getCatalogInstallStatus: async (): Promise<CatalogInstallStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<CatalogInstallStatus>>(
      '/astrometry/catalogs/install'
    );
    if (!data.data) throw new Error(data.error || 'Failed to get catalog install status');
    return data.data;
  },

  installCatalogs: async (preset: CatalogInstallPreset): Promise<CatalogInstallStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<CatalogInstallStatus>>(
      '/astrometry/catalogs/install',
      { preset }
    );
    if (!data.data) throw new Error(data.error || 'Failed to start catalog installation');
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
      remote_image_upload?: {
        enabled: boolean;
        image_directory?: string;
        token?: string;
      };
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

  /** Preview or run a safe scheduler database sync. */
  syncDatabase: async (
    dbId: string,
    req: SchedulerSyncRequest
  ): Promise<SchedulerSyncResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<SchedulerSyncResponse>>(
      `/databases/${encodeURIComponent(dbId)}/sync`,
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to sync databases');
    return data.data;
  },

  /** Create a server-owned dry preview that must be applied by its ID. */
  previewDatabaseSync: async (
    dbId: string,
    req: SchedulerSyncRequest
  ): Promise<SchedulerSyncPreviewResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<SchedulerSyncPreviewResponse>>(
      `/databases/${encodeURIComponent(dbId)}/sync/preview`,
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to preview database sync');
    return data.data;
  },

  /** Reload one durable preview after the Settings UI closes or reloads. */
  getDatabaseSyncPreview: async (
    dbId: string,
    previewId: string
  ): Promise<SchedulerSyncPreviewResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<SchedulerSyncPreviewResponse>>(
      `/databases/${encodeURIComponent(dbId)}/sync/previews/${encodeURIComponent(previewId)}`
    );
    if (!data.data) throw new Error(data.error || 'Database sync preview not found');
    return data.data;
  },

  deleteDatabaseSyncPreview: async (dbId: string, previewId: string): Promise<void> => {
    const apiInstance = await getApi();
    await apiInstance.delete(
      `/databases/${encodeURIComponent(dbId)}/sync/previews/${encodeURIComponent(previewId)}`
    );
  },

  /** Apply one unexpired server-owned preview. */
  applyDatabaseSyncPreview: async (
    dbId: string,
    previewId: string
  ): Promise<SchedulerSyncResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<SchedulerSyncResponse>>(
      `/databases/${encodeURIComponent(dbId)}/sync/previews/${encodeURIComponent(previewId)}/apply`
    );
    if (!data.data) throw new Error(data.error || 'Failed to apply database sync preview');
    return data.data;
  },

  /**
   * Create a brand-new Target Scheduler database (full upstream schema) and
   * start a background import of the given FITS folders.
   */
  createDatabaseFromImages: async (
    req: CreateDatabaseRequest
  ): Promise<CreateDatabaseResponse> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<CreateDatabaseResponse>>(
      '/databases/create',
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to create database');
    return data.data;
  },

  /** Start a background FITS import into an existing database. */
  startImport: async (dbId: string, req: ImportRequest): Promise<ImportStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<ImportStatus>>(
      dbPath(dbId, '/import'),
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to start import');
    return data.data;
  },

  /** Import job progress (poll ~1s while running). */
  getImportStatus: async (dbId: string): Promise<ImportStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ImportStatus>>(dbPath(dbId, '/import'));
    if (!data.data) throw new Error('Failed to get import status');
    return data.data;
  },

  /**
   * Absolute URL for the streaming export zip (non-rejected lights, laid out
   * `<target>/LIGHT/<filter>/…`). Used as a plain download link.
   */
  exportDownloadUrl: (
    dbId: string,
    params: { project_id?: number; target_id?: number; include_pending?: boolean }
  ): string => {
    const query = new URLSearchParams();
    if (params.project_id !== undefined) query.set('project_id', String(params.project_id));
    if (params.target_id !== undefined) query.set('target_id', String(params.target_id));
    if (params.include_pending) query.set('include_pending', 'true');
    const qs = query.toString();
    return withServerUrl(`/api${dbPath(dbId, '/export')}${qs ? `?${qs}` : ''}`);
  },

  /**
   * Run the folder export on the server's own filesystem (desktop mode:
   * server and user share a machine). Hardlinks by default, falling back to
   * copy. Management-gated server-side.
   */
  exportLocal: async (
    dbId: string,
    req: {
      dest: string;
      project_id?: number;
      target_id?: number;
      include_pending?: boolean;
      filter_name?: string;
      link?: boolean;
      dry_run?: boolean;
    }
  ): Promise<{
    planned: number;
    copied: number;
    linked: number;
    skipped_existing: number;
    missing: number;
    errors: number;
    bytes: number;
  }> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<
      ApiResponse<{
        planned: number;
        copied: number;
        linked: number;
        skipped_existing: number;
        missing: number;
        errors: number;
        bytes: number;
      }>
    >(dbPath(dbId, '/export/local'), req);
    if (!data.data) throw new Error(data.error || 'Failed to export');
    return data.data;
  },

  /** Update a project's Target Scheduler fields. */
  updateProject: async (
    dbId: string,
    projectId: number,
    request: ProjectSchedulerUpdate | string
  ): Promise<void> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<{ updated: boolean }>>(
      dbPath(dbId, `/projects/${projectId}`),
      typeof request === 'string' ? { name: request } : request
    );
    if (!data.data) throw new Error(data.error || 'Failed to rename project');
  },

  /** Rename a target and/or move it to another project (same profile). */
  updateTarget: async (
    dbId: string,
    targetId: number,
    req: TargetSchedulerUpdate
  ): Promise<void> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<{ updated: boolean }>>(
      dbPath(dbId, `/targets/${targetId}`),
      req
    );
    if (!data.data) throw new Error(data.error || 'Failed to update target');
  },

  /** Merge a project's targets and images into another project. */
  mergeProject: async (
    dbId: string,
    projectId: number,
    intoProjectId: number
  ): Promise<{ targets_moved: number; images_moved: number }> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<
      ApiResponse<{ targets_moved: number; images_moved: number }>
    >(dbPath(dbId, `/projects/${projectId}/merge`), { into_project_id: intoProjectId });
    if (!data.data) throw new Error(data.error || 'Failed to merge project');
    return data.data;
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

  applyStackStretch: async (
    dbId: string,
    jobId: string,
    groupIndex: number,
    request: StackViewProcessingRequest
  ): Promise<StackStretchPreview> => {
    try {
      const apiInstance = await getApi();
      const { data } = await apiInstance.post<ApiResponse<StackStretchPreview>>(
        dbPath(
          dbId,
          `/stack-previews/${encodeURIComponent(jobId)}/${groupIndex}/stretch`
        ),
        request
      );
      if (!data.data) throw new Error(data.error || 'Failed to apply stack stretch');
      return normalizeStretchPreview(data.data);
    } catch (cause) {
      throw stackStretchError(cause, 'Failed to apply stack stretch');
    }
  },

  getStackColorCatalog: async (
    dbId: string,
    projectId: number
  ): Promise<StackColorCatalog> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<StackColorCatalog>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews/color`)
    );
    if (!data.data) throw new Error(data.error || 'Color preview availability could not be loaded');
    return data.data;
  },

  startStackColor: async (
    dbId: string,
    projectId: number,
    request: {
      target_id: number;
      kind: StackColorKind;
      palette?: StackNarrowbandPalette;
      force?: boolean;
      processing?: StackColorProcessing;
    }
  ): Promise<StackColorJob> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<StackColorJob>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews/color`),
      request
    );
    if (!data.data) throw new Error(data.error || 'Failed to start color preview');
    return data.data;
  },

  getStackColorJob: async (
    dbId: string,
    projectId: number,
    jobId: string
  ): Promise<StackColorJob> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<StackColorJob>>(
      dbPath(dbId, `/projects/${projectId}/stack-previews/color/${encodeURIComponent(jobId)}`)
    );
    if (!data.data) throw new Error(data.error || 'Color preview job not found');
    return data.data;
  },

  getStackColorPreviewUrl: (
    dbId: string,
    jobId: string,
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
      `/stack-previews/color/${encodeURIComponent(jobId)}/preview`
    )}${query}`;
  },

  getStackColorFitsUrl: (
    dbId: string,
    jobId: string,
    artifactRevision: string
  ): string => {
    const serverUrl = getCachedServerUrl();
    const basePath = serverUrl ? `${serverUrl}/api` : '/api';
    const revision = artifactRevision ? `?v=${encodeURIComponent(artifactRevision)}` : '';
    return `${basePath}${dbPath(
      dbId,
      `/stack-previews/color/${encodeURIComponent(jobId)}/fits`
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

  getProjectScheduler: async (
    dbId: string,
    projectId: number
  ): Promise<ProjectSchedulerDetails> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<ProjectSchedulerDetails>>(
      dbPath(dbId, `/projects/${projectId}/scheduler`)
    );
    if (!data.data) throw new Error(data.error || 'Failed to load project schedule');
    return data.data;
  },

  createExposurePlan: async (
    dbId: string,
    targetId: number,
    request: CreateExposurePlanRequest
  ): Promise<ExposurePlanDetails> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<ExposurePlanDetails>>(
      dbPath(dbId, `/targets/${targetId}/exposure-plans`),
      request
    );
    if (!data.data) throw new Error(data.error || 'Failed to create exposure plan');
    return data.data;
  },

  updateExposurePlan: async (
    dbId: string,
    planId: number,
    request: { exposure: number; desired: number; enabled: boolean }
  ): Promise<void> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.put<ApiResponse<{ updated: boolean }>>(
      dbPath(dbId, `/exposure-plans/${planId}`),
      request
    );
    if (!data.data) throw new Error(data.error || 'Failed to update exposure plan');
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

  startQualityBackfill: async (
    dbId: string,
    request: QualityBackfillRequest
  ): Promise<QualityBackfillStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.post<ApiResponse<QualityBackfillStatus>>(
      dbPath(dbId, '/analysis/quality-backfill'),
      request
    );
    if (!data.data) throw new Error('Failed to start database quality analysis');
    return data.data;
  },

  getQualityBackfillStatus: async (dbId: string): Promise<QualityBackfillStatus> => {
    const apiInstance = await getApi();
    const { data } = await apiInstance.get<ApiResponse<QualityBackfillStatus>>(
      dbPath(dbId, '/analysis/quality-backfill')
    );
    if (!data.data) throw new Error('Failed to get database quality status');
    return data.data;
  },
};
