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
export const handlers = [
  // Sequence analysis endpoint
  http.get('/api/db/:dbId/analysis/sequence', () => {
    return HttpResponse.json(emptySequenceAnalysis);
  }),

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
  http.get('/api/db/:dbId/cache-progress', () =>
    HttpResponse.json({
      success: true,
      data: { is_refreshing: false, stage: 'idle' },
      error: null,
      status: 'ready',
    })
  ),
];
