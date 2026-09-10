import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { apiClient } from '../client';

describe('calibration master API', () => {
  it.each(['mono', 'color'] as const)('binds %s catalogs to the exact artifact revision', async (kind) => {
    let received: URL | undefined;
    server.use(http.get('/api/db/:dbId/stack-previews/*/calibration-masters', ({ request }) => {
      received = new URL(request.url);
      return HttpResponse.json({ data: { masters: [], notes: ['No file provenance.'] } });
    }));
    const source = kind === 'mono'
      ? { kind, jobId: 'old job', groupIndex: 3, artifactRevision: 'revision&1' }
      : { kind, jobId: 'old job', artifactRevision: 'revision&1' };
    const result = await apiClient.getStackCalibrationMasters('db one', source);
    expect(received?.pathname).toBe(`/api/db/db%20one/stack-previews/${kind === 'mono' ? 'old%20job/3' : 'color/old%20job'}/calibration-masters`);
    expect(received?.searchParams.get('revision')).toBe('revision&1');
    expect(result.notes).toEqual(['No file provenance.']);
  });

  it('partitions mixed preview requests and restores their original status order', async () => {
    let imageBody: unknown;
    let masterBody: unknown;
    server.use(
      http.post('/api/db/test/images/generation-status', async ({ request }) => {
        imageBody = await request.json();
        return HttpResponse.json({ data: { statuses: [{ state: 'ready' }] } });
      }),
      http.post('/api/db/test/stack-previews/calibration-masters/generation-status', async ({ request }) => {
        masterBody = await request.json();
        return HttpResponse.json({ data: { statuses: [{ state: 'error', error: 'missing' }, { state: 'generating' }] } });
      }),
    );
    const statuses = await apiClient.getGenerationStatus('test', [
      { kind: 'calibration_master', source: { kind: 'mono', jobId: 'mono', groupIndex: 4, artifactRevision: 'r1' }, masterId: 'master-one', size: 'original', midtone: 0.3, shadow: -2 },
      { kind: 'preview', imageId: 7, size: 'screen', color: true },
      { kind: 'calibration_master', source: { kind: 'color', jobId: 'color', artifactRevision: 'r2' }, masterId: 'master-two', size: 'screen' },
    ]);
    expect(statuses).toEqual([{ state: 'error', error: 'missing' }, { state: 'ready' }, { state: 'generating' }]);
    expect(imageBody).toEqual({ requests: [{ kind: 'preview', image_id: 7, size: 'screen', color: true }] });
    expect(masterBody).toEqual({ requests: [
      { source: { kind: 'mono', job_id: 'mono', group_index: 4, artifact_revision: 'r1' }, master_id: 'master-one', size: 'original', midtone: 0.3, shadow: -2 },
      { source: { kind: 'color', job_id: 'color', artifact_revision: 'r2' }, master_id: 'master-two', size: 'screen' },
    ] });
  });

  it.each(['image', 'master'])('retains successful statuses when the %s endpoint fails', async (failed) => {
    server.use(
      http.post('/api/db/test/images/generation-status', () => failed === 'image'
        ? HttpResponse.json({ error: 'Image queue unavailable' }, { status: 503 })
        : HttpResponse.json({ data: { statuses: [{ state: 'ready' }] } })),
      http.post('/api/db/test/stack-previews/calibration-masters/generation-status', () => failed === 'master'
        ? HttpResponse.json({ error: 'Master queue unavailable' }, { status: 503 })
        : HttpResponse.json({ data: { statuses: [{ state: 'ready' }] } })),
    );
    const statuses = await apiClient.getGenerationStatus('test', [
      { kind: 'preview', imageId: 7, size: 'screen' },
      { kind: 'calibration_master', source: { kind: 'color', jobId: 'color', artifactRevision: 'r2' }, masterId: 'master-two', size: 'screen' },
    ]);
    expect(statuses).toEqual(failed === 'image'
      ? [{ state: 'generating' }, { state: 'ready' }]
      : [{ state: 'ready' }, { state: 'generating' }]);
  });

  it('starts both endpoints without waiting for the image queue to respond', async () => {
    let releaseImages!: () => void;
    const imagesReleased = new Promise<void>((resolve) => { releaseImages = resolve; });
    server.use(
      http.post('/api/db/test/images/generation-status', async () => {
        await imagesReleased;
        return HttpResponse.json({ data: { statuses: [{ state: 'ready' }] } });
      }),
      http.post('/api/db/test/stack-previews/calibration-masters/generation-status', () => {
        releaseImages();
        return HttpResponse.json({ data: { statuses: [{ state: 'ready' }] } });
      }),
    );
    expect(await apiClient.getGenerationStatus('test', [
      { kind: 'preview', imageId: 7, size: 'screen' },
      { kind: 'calibration_master', source: { kind: 'color', jobId: 'color', artifactRevision: 'r2' }, masterId: 'master-two', size: 'screen' },
    ])).toEqual([{ state: 'ready' }, { state: 'ready' }]);
  });
});
