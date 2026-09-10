import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockInstance,
} from 'vitest';
import {
  registerPending,
  descriptorKey,
  __resetForTest,
} from '../previewPoll';
import { apiClient } from '../../api/client';
import type { PreviewDescriptor, GenerationStatus, CalibrationMasterPreviewDescriptor } from '../../api/types';

const preview = (imageId: number): PreviewDescriptor => ({
  imageId,
  kind: 'preview',
  size: 'screen',
});

let statusSpy: MockInstance<typeof apiClient.getGenerationStatus>;

/** Make getGenerationStatus reply with `map(descriptor)` for each request. */
function respondWith(map: (d: PreviewDescriptor) => GenerationStatus) {
  statusSpy.mockImplementation(async (_dbId, requests) => requests.map(map));
}

describe('previewPoll coordinator', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    statusSpy = vi.spyOn(apiClient, 'getGenerationStatus');
  });

  afterEach(() => {
    __resetForTest();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('resolves onReady, then stops polling once idle', async () => {
    respondWith(() => ({ state: 'ready' }));
    const onReady = vi.fn();
    registerPending('db1', preview(1), { onReady, onError: vi.fn() });

    await vi.advanceTimersByTimeAsync(50); // fire the immediate poll
    expect(onReady).toHaveBeenCalledTimes(1);

    // Entry removed → timer stops → no further polls.
    const callsAfterReady = statusSpy.mock.calls.length;
    await vi.advanceTimersByTimeAsync(2000);
    expect(statusSpy.mock.calls.length).toBe(callsAfterReady);
  });

  it('coalesces multiple pending images into a single batched request', async () => {
    respondWith(() => ({ state: 'generating' })); // keep them pending
    registerPending('db1', preview(1), { onReady: vi.fn(), onError: vi.fn() });
    registerPending('db1', preview(2), { onReady: vi.fn(), onError: vi.fn() });
    registerPending('db1', preview(3), { onReady: vi.fn(), onError: vi.fn() });

    // Immediate poll + one interval tick: a tick sees all three pending.
    await vi.advanceTimersByTimeAsync(900);

    const maxBatch = Math.max(
      0,
      ...statusSpy.mock.calls.map(([, requests]) => requests.length)
    );
    expect(maxBatch).toBeGreaterThan(1);
    // Every batched request is scoped to the one database.
    for (const [dbId] of statusSpy.mock.calls) expect(dbId).toBe('db1');
  });

  it('dedups identical descriptors but notifies every subscriber', async () => {
    respondWith(() => ({ state: 'ready' }));
    const a = vi.fn();
    const b = vi.fn();
    registerPending('db1', preview(7), { onReady: a, onError: vi.fn() });
    registerPending('db1', preview(7), { onReady: b, onError: vi.fn() });

    await vi.advanceTimersByTimeAsync(50);

    // One descriptor in the request (deduped by key) …
    expect(statusSpy.mock.calls[0][1]).toHaveLength(1);
    // … but both subscribers fired.
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it('reports terminal errors via onError', async () => {
    respondWith(() => ({ state: 'error', error: 'source file not found' }));
    const onReady = vi.fn();
    const onError = vi.fn();
    registerPending('db1', preview(9), { onReady, onError });

    await vi.advanceTimersByTimeAsync(50);
    expect(onError).toHaveBeenCalledWith('source file not found');
    expect(onReady).not.toHaveBeenCalled();
  });

  it('unregister before resolution prevents the callback and stops the timer', async () => {
    respondWith(() => ({ state: 'generating' }));
    const onReady = vi.fn();
    const unregister = registerPending('db1', preview(5), {
      onReady,
      onError: vi.fn(),
    });
    unregister();

    await vi.advanceTimersByTimeAsync(2000);
    expect(onReady).not.toHaveBeenCalled();
    // Nothing pending → after the in-flight immediate poll the timer is idle.
    const calls = statusSpy.mock.calls.length;
    await vi.advanceTimersByTimeAsync(2000);
    expect(statusSpy.mock.calls.length).toBe(calls);
  });

  it('descriptorKey distinguishes size and kind', () => {
    expect(descriptorKey(preview(1))).not.toBe(
      descriptorKey({ imageId: 1, kind: 'annotated', size: 'screen' })
    );
    expect(descriptorKey(preview(1))).not.toBe(
      descriptorKey({ imageId: 1, kind: 'preview', size: 'large' })
    );
  });

  it('keys calibration previews by exact provenance and display stretch', () => {
    const master: CalibrationMasterPreviewDescriptor = {
      kind: 'calibration_master', masterId: 'same-master', size: 'original', midtone: 0.2,
      source: { kind: 'mono', jobId: 'job', groupIndex: 0, artifactRevision: 'old' },
    };
    const variants: CalibrationMasterPreviewDescriptor[] = [
      { ...master, source: { ...master.source, artifactRevision: 'new' } },
      { ...master, source: { kind: 'mono', jobId: 'job', groupIndex: 1, artifactRevision: 'old' } },
      { ...master, source: { kind: 'color', jobId: 'job', artifactRevision: 'old' } },
      { ...master, masterId: 'another-master' },
      { ...master, size: 'screen' },
      { ...master, midtone: 0.4 },
      { ...master, shadow: -2 },
    ];
    expect(new Set([master, ...variants].map(descriptorKey)).size).toBe(8);
  });

  it('drops an old master subscriber when selection changes while polling', async () => {
    let resolveStatus!: (statuses: GenerationStatus[]) => void;
    statusSpy.mockImplementationOnce(() => new Promise((resolve) => { resolveStatus = resolve; }))
      .mockResolvedValue([{ state: 'ready' }]);
    const master: CalibrationMasterPreviewDescriptor = {
      kind: 'calibration_master', masterId: 'old', size: 'original',
      source: { kind: 'color', jobId: 'job', artifactRevision: 'r' },
    };
    const oldReady = vi.fn();
    const newReady = vi.fn();
    const unregister = registerPending('db1', master, { onReady: oldReady, onError: vi.fn() });
    unregister();
    registerPending('db1', { ...master, masterId: 'new' }, { onReady: newReady, onError: vi.fn() });
    resolveStatus([{ state: 'ready' }]);
    await vi.advanceTimersByTimeAsync(900);
    expect(oldReady).not.toHaveBeenCalled();
    expect(newReady).toHaveBeenCalledTimes(1);
  });
});
