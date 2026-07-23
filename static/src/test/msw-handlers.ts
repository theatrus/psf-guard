import { http, HttpResponse } from 'msw';

// Default empty responses for handlers
const emptySequenceAnalysis = {
  success: true,
  data: { sequences: [] },
  error: null,
  status: 'ready',
};

const notFoundImageQuality = {
  success: false,
  data: null,
  error: 'Image not found',
  status: null,
};

const emptyList = {
  success: true,
  data: [],
  error: null,
  status: 'ready',
};

// Default handlers for every endpoint a component might fall through to
// without registering its own mock. Path-nested under `/api/db/:dbId/...`
// to match production routing (see B2 in MULTI_DB_PLAN.md). Per-test
// `server.use(...)` overrides win for whatever the spec cares about; these
// fill in the gaps so MSW's `onUnhandledRequest: 'error'` doesn't trip
// every time a side-effect query fires.
const idleSpatialScan = {
  success: true,
  data: {
    started: false,
    progress: {
      running: false,
      target_id: null,
      filter_name: null,
      total: 0,
      processed: 0,
      skipped_cached: 0,
      errors: 0,
      current_file: null,
      started_at: null,
      finished_at: null,
      last_error: null,
    },
    cached_count: 0,
  },
  error: null,
  status: 'ready',
};

export const handlers = [
  // Sequence analysis endpoint
  http.get('/api/db/:dbId/analysis/sequence', () => {
    return HttpResponse.json(emptySequenceAnalysis);
  }),

  // Spatial (occlusion) scan endpoints
  http.get('/api/db/:dbId/analysis/quality-scan', () => {
    return HttpResponse.json(idleSpatialScan);
  }),
  http.post('/api/db/:dbId/analysis/quality-scan', () => {
    return HttpResponse.json(idleSpatialScan);
  }),
  http.get('/api/db/:dbId/analysis/quality-backfill', () =>
    HttpResponse.json({
      success: true,
      data: {
        started: false,
        progress: {
          running: false,
          force: false,
          total_targets: 0,
          processed_targets: 0,
          current_target_id: null,
          started_at: null,
          finished_at: null,
        },
      },
      error: null,
      status: 'ready',
    })
  ),

  // Image quality endpoint
  http.get('/api/db/:dbId/analysis/image/:imageId', () => {
    return HttpResponse.json(notFoundImageQuality, { status: 404 });
  }),

  // Per-DB project / target / image listings
  http.get('/api/db/:dbId/projects', () => HttpResponse.json(emptyList)),
  http.get('/api/db/:dbId/projects/overview', () => HttpResponse.json(emptyList)),
  http.get('/api/db/:dbId/targets/overview', () => HttpResponse.json(emptyList)),
  http.get('/api/db/:dbId/projects/:projectId/targets', () =>
    HttpResponse.json(emptyList)
  ),
  http.get('/api/db/:dbId/images', () => HttpResponse.json(emptyList)),
  http.get('/api/db/:dbId/images/:imageId', () =>
    HttpResponse.json({
      success: false,
      data: null,
      error: 'Image not found',
      status: null,
    }, { status: 404 })
  ),

  // Grade update
  http.put('/api/db/:dbId/images/:imageId/grade', () => {
    return HttpResponse.json({
      success: true,
      data: null,
      error: null,
      status: 'ready',
    });
  }),

  // Cross-DB endpoints
  http.get('/api/databases', () => HttpResponse.json(emptyList)),
  http.get('/api/info', () =>
    HttpResponse.json({
      success: true,
      data: {
        version: 'test',
        cache_directory: '/tmp/cache',
        allow_database_management: false,
      },
      error: null,
      status: 'ready',
    })
  ),
  http.get('/api/astrometry/capabilities', () =>
    HttpResponse.json({
      success: true,
      data: {
        seiza_version: 'test',
        seiza_fits_version: 'test',
        resources: {
          objects: { name: 'objects', status: 'not_configured' },
          stars: { name: 'stars', status: 'not_configured' },
          star_identifiers: { name: 'star_identifiers', status: 'not_configured' },
          blind_index: { name: 'blind_index', status: 'not_configured' },
          transients: { name: 'transients', status: 'not_configured' },
          minor_bodies: { name: 'minor_bodies', status: 'not_configured' },
        },
        features: {
          object_association: false,
          object_name_search: false,
          stellar_name_search: false,
          hinted_solve: false,
          blind_solve: false,
          transient_annotations: false,
          minor_body_annotations: false,
        },
      },
      error: null,
      status: 'ready',
    })
  ),
  http.get('/api/astrometry/catalogs/install', () =>
    HttpResponse.json({
      success: true,
      data: {
        started: false,
        progress: {
          running: false,
          phase: 'idle',
          message: 'No catalog installation has run.',
          files_completed: 0,
          files_total: 0,
        },
      },
      error: null,
      status: 'ready',
    })
  ),
  http.get('/api/db/:dbId/cache-progress', () =>
    HttpResponse.json({
      success: true,
      data: { is_refreshing: false, stage: 'idle' },
      error: null,
      status: 'ready',
    })
  ),
];
