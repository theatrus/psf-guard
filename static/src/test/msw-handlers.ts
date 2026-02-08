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

export const handlers = [
  // Sequence analysis endpoint
  http.get('/api/analysis/sequence', () => {
    return HttpResponse.json(emptySequenceAnalysis);
  }),

  // Image quality endpoint
  http.get('/api/analysis/image/:imageId', () => {
    return HttpResponse.json(notFoundImageQuality, { status: 404 });
  }),

  // Projects endpoint (needed for SequenceView)
  http.get('/api/projects', () => {
    return HttpResponse.json({
      success: true,
      data: [],
      error: null,
      status: 'ready',
    });
  }),

  // Targets for project
  http.get('/api/projects/:projectId/targets', () => {
    return HttpResponse.json({
      success: true,
      data: [],
      error: null,
      status: 'ready',
    });
  }),

  // Images list
  http.get('/api/images', () => {
    return HttpResponse.json({
      success: true,
      data: [],
      error: null,
      status: 'ready',
    });
  }),

  // Grade update
  http.put('/api/images/:imageId/grade', () => {
    return HttpResponse.json({
      success: true,
      data: null,
      error: null,
      status: 'ready',
    });
  }),
];
